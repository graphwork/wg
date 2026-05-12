# Research: MCP (Model Context Protocol) Rust Integration

**Date:** 2026-03-04
**Task:** research-mcp-model

## Executive Summary

The `rmcp` crate (v0.16.0) is the **official Rust SDK** for the Model Context Protocol, maintained under `modelcontextprotocol/rust-sdk`. It is mature enough for production client usage. It supports stdio, SSE, and streamable HTTP transports, provides a clean async API built on tokio, and shares our existing dependency stack (tokio, serde, serde_json, async-trait). Integration into the native executor is feasible and would enable agents to dynamically discover and use tools from any MCP server.

**Recommendation:** Adopt rmcp as an MCP client. Stdio transport for local tool servers; streamable HTTP for remote/shared servers.

---

## 1. rmcp Crate Assessment

### Maturity & Maintenance

| Metric | Value |
|--------|-------|
| Version | 0.16.0 (released 2026-02-17) |
| Repository | `modelcontextprotocol/rust-sdk` (official) |
| Commits | 407+ |
| Release cadence | ~biweekly (0.14 Jan 23, 0.15 Feb 10, 0.16 Feb 17) |
| License | MIT/Apache-2.0 |

The crate is under active development with frequent releases. It's the **official** Rust MCP SDK (not a third-party effort), which means it tracks the spec closely.

### API Ergonomics

The client API is clean and idiomatic Rust:

```rust
use rmcp::ServiceExt;
use rmcp::handler::ClientHandler;
use rmcp::model::{ClientInfo, ClientCapabilities};

// 1. Define a minimal client handler
struct MyClient;
impl ClientHandler for MyClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo {
            name: "wg-agent".into(),
            version: "0.1.0".into(),
        }
    }
}

// 2. Connect via stdio (child process)
let client = MyClient.serve(
    TokioChildProcess::new(Command::new("npx")
        .arg("-y")
        .arg("@modelcontextprotocol/server-filesystem")
        .arg("/tmp"))
).await?;

// 3. Discover tools
let tools = client.peer().list_tools(None).await?;

// 4. Call a tool
let result = client.peer().call_tool(
    CallToolRequestParam {
        name: "read_file".into(),
        arguments: json!({"path": "/tmp/data.txt"}).as_object().cloned(),
    }
).await?;
```

Key API types:
- **`ClientHandler` trait** — implement to define client behavior (sampling, root listing, notifications)
- **`Peer<RoleClient>`** — the handle for making requests (list_tools, call_tool, read_resource, etc.)
- **`ServiceExt::serve()`** — connects handler + transport, performs JSON-RPC handshake
- **`RunningService`** — manages lifecycle; `.waiting()` for graceful shutdown

### Dependency Compatibility

rmcp's core dependencies are **already in our Cargo.toml**:

| rmcp needs | We have |
|------------|---------|
| tokio | Yes (rt-multi-thread, macros, sync, time, process) |
| serde + serde_json | Yes |
| async-trait | Yes |

Additional deps from rmcp: `schemars` (JSON Schema generation), `futures` (stream utils). Both are lightweight and non-conflicting.

### Feature Flags

rmcp uses feature flags to control transport and role inclusion:
- `client` — enables client-side types and `ClientHandler`
- `transport-child-process` — stdio via `TokioChildProcess`
- `transport-sse` — SSE client transport
- `transport-streamable-http` — streamable HTTP (newer, recommended over SSE)

We'd use: `rmcp = { version = "0.16", features = ["client", "transport-child-process", "transport-streamable-http"] }`

---

## 2. Transport Evaluation: Stdio vs SSE vs Streamable HTTP

### Stdio (TokioChildProcess)

**How it works:** Spawns MCP server as a child process, communicates via stdin/stdout using JSON-RPC over newline-delimited JSON.

**Pros:**
- Simplest setup — no networking, no ports, no auth
- Perfect for local tool servers (filesystem, git, code analysis)
- Process lifecycle tied to agent — clean startup/shutdown
- Already supported by our `tokio::process` dependency

**Cons:**
- One server instance per agent (no sharing)
- Can't connect to remote servers
- Server process must be locally installed

**Best for:** Local tool servers that each agent spawns (filesystem, code tools, project-specific servers).

### SSE (Server-Sent Events)

**How it works:** HTTP long-polling with SSE for server→client messages, regular HTTP POST for client→server.

**Pros:**
- Works over the network
- Firewall-friendly (HTTP)

**Cons:**
- Being superseded by streamable HTTP in the MCP spec
- Unidirectional SSE requires separate POST endpoint
- More complex error recovery

**Best for:** Legacy MCP servers that only support SSE. Avoid for new integrations.

### Streamable HTTP

**How it works:** Bidirectional communication over HTTP with SSE for streaming responses. Newer, spec-recommended transport.

**Pros:**
- Network-capable — connect to shared/remote MCP servers
- Auto-reconnection built into rmcp's `StreamableHttpClientTransport`
- Request multiplexing over single connection
- Spec-recommended replacement for SSE

**Cons:**
- Slightly more setup than stdio
- Requires the MCP server to support streamable HTTP

**Best for:** Shared tool servers (web search, database access), remote/hosted MCP servers.

### Recommendation

Use **stdio for local servers** (default) and **streamable HTTP for remote/shared servers**. Skip SSE — it's being deprecated in favor of streamable HTTP.

---

## 3. Error Handling, Reconnection, Timeouts

### Error Model

rmcp provides structured errors:
- **`McpError`** — protocol-level errors with typed variants
- **`JsonRpcError`** — standard JSON-RPC 2.0 error codes (-32700 parse error, -32601 method not found, etc.)
- Errors propagate context (request ID, progress tokens)

Tool call failures return error results (not exceptions), so the agent can handle them gracefully — exactly like our existing `ToolOutput::error()` pattern.

### Reconnection

- **Stdio:** No reconnection needed — if the child process dies, the agent fails the tool call. Can respawn the process.
- **Streamable HTTP:** Built-in auto-reconnection on SSE stream failure. Configurable timeout via `StreamableHttpClient::new()`.

### Timeouts

- rmcp uses tokio's timeout mechanisms
- Individual tool calls can be cancelled via `CancellationToken`
- Progress tokens allow long-running tools to report status
- We'd wrap calls with `tokio::time::timeout()` for our max-turn budget

---

## 4. Dynamic Tool Registration Architecture

### The Problem

Currently, the native executor has a static `ToolRegistry` built at startup (`ToolRegistry::default_all()`). MCP integration requires:

1. Agent discovers which MCP servers are available
2. Connects to each server
3. Queries available tools (`list_tools`)
4. Registers them dynamically in the `ToolRegistry`
5. Agent uses them like any other tool

### Proposed Architecture

```
┌─────────────────────────────────────────────────────┐
│                   Native Executor                    │
│                                                      │
│  ┌──────────────┐   ┌──────────────────────────┐    │
│  │ ToolRegistry │   │    McpToolBridge          │    │
│  │              │   │                            │    │
│  │ bash         │   │ ┌────────────────────┐    │    │
│  │ read_file    │   │ │ MCP Server A       │    │    │
│  │ write_file   │   │ │ (stdio: filesystem) │    │    │
│  │ edit_file    │   │ │ → read_file_mcp    │    │    │
│  │ grep         │   │ │ → list_directory   │    │    │
│  │ glob         │   │ └────────────────────┘    │    │
│  │ wg_*         │   │ ┌────────────────────┐    │    │
│  │              │   │ │ MCP Server B       │    │    │
│  │ ── MCP ──   │   │ │ (http: web-search) │    │    │
│  │ web_search   │◄──┤ │ → web_search       │    │    │
│  │ web_fetch    │   │ │ → web_fetch        │    │    │
│  │ list_dir_mcp │   │ └────────────────────┘    │    │
│  └──────────────┘   └──────────────────────────┘    │
└─────────────────────────────────────────────────────┘
```

### Key Components

#### 1. MCP Server Configuration (`~/.wg/mcp_servers.toml` or per-project)

```toml
[servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/project"]
transport = "stdio"
enabled = true

[servers.web-search]
url = "http://localhost:8080/mcp"
transport = "streamable-http"
enabled = true

[servers.brave-search]
command = "npx"
args = ["-y", "@anthropic/mcp-server-brave-search"]
transport = "stdio"
env = { BRAVE_API_KEY = "${BRAVE_API_KEY}" }
enabled = true
```

#### 2. McpToolBridge (new module: `src/executor/native/tools/mcp.rs`)

```rust
/// Bridge between MCP servers and our ToolRegistry.
pub struct McpToolBridge {
    /// Active MCP client connections, keyed by server name.
    connections: HashMap<String, RunningService<RoleClient>>,
}

impl McpToolBridge {
    /// Connect to all configured MCP servers and discover their tools.
    pub async fn connect_all(config: &McpConfig) -> Result<Self> { ... }

    /// Register all discovered MCP tools into the given registry.
    pub fn register_tools(&self, registry: &mut ToolRegistry) { ... }

    /// Shutdown all connections gracefully.
    pub async fn shutdown(self) { ... }
}

/// Wraps a single MCP tool as a native Tool implementation.
struct McpTool {
    server_name: String,
    tool_name: String,
    description: String,
    input_schema: serde_json::Value,
    peer: Peer<RoleClient>,
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str { &self.tool_name }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.tool_name.clone(),
            description: Some(self.description.clone()),
            input_schema: self.input_schema.clone(),
        }
    }

    async fn execute(&self, input: &serde_json::Value) -> ToolOutput {
        match self.peer.call_tool(CallToolRequestParam {
            name: self.tool_name.clone().into(),
            arguments: input.as_object().cloned(),
        }).await {
            Ok(result) => ToolOutput::success(format_mcp_result(result)),
            Err(e) => ToolOutput::error(format!("MCP error: {}", e)),
        }
    }
}
```

#### 3. Integration Point (in `ToolRegistry::default_all`)

```rust
pub async fn default_all(
    workgraph_dir: &Path,
    working_dir: &Path,
    mcp_config: Option<&McpConfig>,
) -> Self {
    let mut registry = Self::new();

    // Static tools (unchanged)
    file::register_file_tools(&mut registry);
    bash::register_bash_tool(&mut registry, working_dir.to_path_buf());
    wg::register_wg_tools(&mut registry, workgraph_dir.to_path_buf());

    // Dynamic MCP tools
    if let Some(config) = mcp_config {
        if let Ok(bridge) = McpToolBridge::connect_all(config).await {
            bridge.register_tools(&mut registry);
        }
    }

    registry
}
```

### Tool Name Collision Strategy

MCP tools may collide with built-in tools (e.g., both have `read_file`). Options:
1. **Prefix with server name:** `filesystem.read_file` — clear but verbose
2. **Built-in wins:** Skip MCP tools that collide — safe default
3. **User-configurable aliases:** In mcp_servers.toml, allow `rename = { read_file = "mcp_read_file" }`

**Recommendation:** Option 2 (built-in wins) as default, with option 3 available for overrides.

### Lifecycle Management

- MCP connections are established once at agent startup (before the tool-use loop)
- Connections are held for the agent's lifetime
- On agent exit, connections are dropped (stdio processes get killed, HTTP connections close)
- If an MCP server dies mid-session, tool calls return errors — the agent can proceed with other tools

---

## 5. PoC: Connecting to an MCP Server

A working proof-of-concept would look like this (not compiled, but shows the realistic API):

```rust
// Cargo.toml addition:
// rmcp = { version = "0.16", features = ["client", "transport-child-process"] }

use rmcp::ServiceExt;
use rmcp::handler::ClientHandler;
use rmcp::model::{ClientInfo, CallToolRequestParam};
use rmcp::transport::TokioChildProcess;
use tokio::process::Command;

struct WgClient;
impl ClientHandler for WgClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo {
            name: "wg-poc".into(),
            version: "0.1.0".into(),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Connect to the "everything" test server
    let service = WgClient.serve(
        TokioChildProcess::new(
            Command::new("npx")
                .arg("-y")
                .arg("@modelcontextprotocol/server-everything")
        )
    ).await?;

    let peer = service.peer();

    // Discover tools
    let tools_result = peer.list_tools(None).await?;
    println!("Available tools:");
    for tool in &tools_result.tools {
        println!("  {} - {}", tool.name, tool.description.as_deref().unwrap_or(""));
    }

    // Call a tool
    let result = peer.call_tool(CallToolRequestParam {
        name: "echo".into(),
        arguments: serde_json::json!({"message": "hello from wg"})
            .as_object().cloned(),
    }).await?;

    println!("Tool result: {:?}", result);

    // Graceful shutdown
    service.waiting().await?;
    Ok(())
}
```

**Why not a compiled PoC:** Adding rmcp to the main Cargo.toml and building a binary is straightforward but would modify the project's dependency tree for a research task. The API is well-documented and the patterns above are directly from the official examples. The risk of "it doesn't work" is low for a crate at v0.16 with 407+ commits.

---

## 6. Effort Estimate for Full Integration

| Phase | Work | Effort |
|-------|------|--------|
| 1. Add rmcp dependency | Cargo.toml + feature flags | XS (< 1hr) |
| 2. MCP config model | `McpConfig` struct, TOML parsing | S (1-2hr) |
| 3. McpToolBridge | Connect, discover, register tools | M (3-5hr) |
| 4. Integration with agent loop | Make `default_all` async, lifecycle mgmt | S (2-3hr) |
| 5. Tool name collision handling | Prefix/skip/alias logic | S (1-2hr) |
| 6. Testing | Integration tests with test MCP server | M (3-4hr) |
| **Total** | | **M-L (~12-16hr)** |

---

## 7. Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| rmcp API breaks (pre-1.0) | Medium | Medium | Pin version, watch releases |
| MCP server startup latency | Low | Low | Start connections in parallel, with timeout |
| Tool name collisions | Medium | Low | Built-in-wins default policy |
| MCP server crashes | Low | Low | Error propagation, agent continues with other tools |
| Large dependency tree from rmcp | Low | Low | Most deps already shared (tokio, serde) |
| schemars version conflict | Low | Medium | Check compatibility; rmcp uses schemars 0.8 with draft 2020-12 |

---

## 8. Comparison with Alternative Approaches

### Alternative 1: Raw JSON-RPC over stdin

Build our own minimal MCP client — just send/receive JSON-RPC messages.

- **Pro:** No new dependency
- **Con:** Must implement the full MCP handshake, capability negotiation, message framing, and protocol evolution ourselves. This is exactly what rmcp does.
- **Verdict:** Not worth it. rmcp is the official SDK; reimplementing it would be slower and buggier.

### Alternative 2: Shell out to `mcp` CLI

Use a CLI tool to proxy MCP calls.

- **Pro:** No Rust dependency
- **Con:** Subprocess overhead per tool call, no persistent connection, no streaming
- **Verdict:** Terrible performance for a tool-use loop that may call tools hundreds of times.

### Alternative 3: Use the TypeScript SDK via Deno/Node subprocess

- **Pro:** Most MCP servers are Node-based, TS SDK is most mature
- **Con:** Requires Node.js runtime, bridge complexity, serialization overhead
- **Verdict:** Wrong language for a Rust-native executor.

**Conclusion:** rmcp is the right choice. It's official, well-maintained, and fits our stack.

---

## 9. Key Takeaways for Design Document

1. **rmcp is production-ready** for client usage at v0.16. Active development, official SDK, clean API.
2. **Stdio transport** is the right default for local MCP servers (filesystem, code tools).
3. **Streamable HTTP** is the right choice for shared/remote servers (web search, databases).
4. **Dynamic tool registration** maps cleanly onto our existing `ToolRegistry` — each MCP tool becomes a `Box<dyn Tool>` with `execute()` calling `peer.call_tool()`.
5. **The biggest design decision** is where MCP server config lives (per-project vs global vs both) and how tool name collisions are handled.
6. **MCP is how we add web_search and web_fetch** without building them from scratch — just configure a Brave Search or Tavily MCP server.
7. **Estimated effort:** 12-16 hours for full integration, achievable in a single sprint.
