# Pan-Executor Architecture: Workgraph as a Self-Sufficient Execution Universe

## Status

**Phase 1 (Stream Capture) complete.** Research phase complete. Ready for Phase 2 implementation.

Last updated: 2026-03-04

---

## 1. Current State

### Executor landscape

Workgraph has **four executor types**, all sharing a fire-and-forget subprocess model:

| Executor | Runtime | Model Access | Tool Provision | Streaming | Status |
|---|---|---|---|---|---|
| **Claude CLI** | `claude -p` subprocess | Anthropic (subscription) | Claude Code built-in (~10 tools) | JSONL → `raw_stream.jsonl` (translated) | Production |
| **Amplifier** | `amplifier run` subprocess | Any (OpenRouter) | Configurable bundles | Init + Result bookends | Production |
| **Native** | `wg native-exec` (Rust binary) | Anthropic API + OpenAI-compat | In-process tools (12: file/bash/wg) | `stream.jsonl` (native StreamEvent) | Working, limited tools |
| **Shell** | `bash -c` subprocess | N/A | N/A | Init + Result bookends | Production |

**Architecture:** All executors are spawned as detached processes via `setsid()`. The coordinator writes a wrapper script (`run.sh`), spawns `bash run.sh`, and polls for completion. The wrapper handles output capture, exit code checking, and `wg done`/`wg fail` on exit.

### Stream capture (Phase 1 — complete)

All executors now produce unified NDJSON events to `<agent_dir>/stream.jsonl` via the `StreamEvent` enum in `src/stream_event.rs`:

- **Init** — session metadata (executor type, model, session ID)
- **Turn** — tool-use loop turn (tools used, token usage)
- **ToolStart/ToolEnd** — tool execution lifecycle
- **Heartbeat** — periodic liveness signal
- **Result** — final aggregated usage and outcome

The coordinator reads these files incrementally via `AgentStreamState` for liveness detection, cost tracking, and progress monitoring. Claude CLI output is translated from its native JSONL format via `translate_claude_event()`.

### Native executor capabilities

The native executor (`src/executor/native/`) is the most strategically important piece:

**LLM clients:**
- `AnthropicClient` — Anthropic Messages API with streaming SSE, extended thinking
- `OpenAiClient` — OpenAI chat completions with tool-use translation; works with OpenRouter, Ollama, vLLM, any OpenAI-compat endpoint

**Tools (12 total):**
- File I/O: `bash`, `read_file`, `write_file`, `list_files`
- Workgraph: `wg_log`, `wg_artifact`, `wg_done`, `wg_fail`, `wg_show`, `wg_add`, `wg_msg_send`, `wg_msg_read`

**Agent loop:** Standard tool-use loop with turn limiting, usage tracking, NDJSON logging via `StreamWriter`.

### What amplifier provides (that native doesn't)

1. **Bundles** — Pre-configured tool sets. Agents get exactly the tools they need.
2. **Multi-model routing** — OpenRouter integration, provider selection
3. **Multi-agent delegation** — Sub-agents with different tool sets
4. **Cost tracking** — Per-invocation cost via OpenRouter
5. **Web tools** — Web search, web fetch

Amplifier does NOT provide: structured stream output, bidirectional communication, liveness detection, or task graph awareness.

---

## 2. Tool Gap Analysis

*Source: [native-executor-tool-gaps.md](research/native-executor-tool-gaps.md) — 23,129 tool calls across 1,363 agents.*

### Coverage

| Tool | Calls | Pct | Native Has? |
|------|------:|----:|:-----------:|
| Bash | 10,352 | 44.8% | Yes |
| Read | 5,856 | 25.3% | Yes |
| Grep | 2,971 | 12.8% | Yes |
| Edit | 2,521 | 10.9% | Yes |
| TodoWrite | 777 | 3.4% | No (N/A) |
| Glob | 190 | 0.8% | Yes |
| WebSearch | 188 | 0.8% | No |
| Write | 185 | 0.8% | Yes |
| WebFetch | 83 | 0.4% | No |

**Current native tool call coverage: 93.8%.** The Bash tool is a universal fallback covering cargo, git, and ad-hoc commands.

### Remaining gaps (prioritized)

| Priority | Tool | Effort | Impact |
|:--------:|------|:------:|--------|
| 1 | `list_dir` | XS | Eliminates ~255 `ls` bash calls |
| 2 | `wg_msg` (read/send combined) | S | Required by standard agent workflow |
| 3 | `wg_context` | S | Agent orientation at task start |
| 4 | `web_search` | L | Required for ~10% of task types (research) |
| 5 | `web_fetch` | M | HTTP GET + HTML-to-markdown for docs/API research |

### Tools NOT worth adding as native

- **`git` tools**: Git porcelain is complex; Bash handles it well (~1,100 calls, all fine through Bash)
- **`cargo` tools**: Same reasoning (~2,100 calls through Bash)
- **`todo_write`**: Claude Code UX feature, not meaningful for native agents
- **Agency tools** (`wg agent/role/assign`): Only used by assignment tasks; Bash suffices

### Assessment

Adding `list_dir` and completing `wg_msg` → **~96% coverage**. Adding `web_search` and `web_fetch` → **~97% coverage, ~98% task type coverage**. The only task category not well-served without web tools is research tasks (~10% of tasks).

---

## 3. Web Search Recommendation

*Source: [web-search-api-comparison.md](research/web-search-api-comparison.md) — 6 APIs evaluated.*

### Recommendation: Serper (primary), SearXNG (self-hosted fallback)

| Criterion | Serper | Brave | Tavily | SearXNG |
|-----------|--------|-------|--------|---------|
| Cost/1k queries | $0.30–$1.00 | $5–$9 | $1–$2 | Free |
| Result quality | Excellent (Google) | Good | Good (AI-curated) | Variable |
| Rate limit | 300 req/s | 20–50 req/s | 100 RPM | Unlimited |
| Self-hosted | No | No | No | Yes |

**Why Serper:** Cheapest (5–16x cheaper than Brave), best quality (actual Google results), simplest integration (single REST endpoint), highest rate limit (300 req/s), no subscription lock-in.

**Cost projection:** At 100 agents/day with 0.14 searches/agent → $0.014/day. Negligible even at 10,000 agents/day ($1.40/day).

### Architecture

```
web_search tool
  ├── SerperBackend (default, API key required)
  ├── BraveBackend (alternative)
  ├── TavilyBackend (alternative)
  └── SearXNGBackend (self-hosted, URL required)
```

Configured via `WG_SEARCH_BACKEND` + `WG_SEARCH_API_KEY` env vars or `.wg/config.toml`.

**Alternative approach:** Use an MCP server for web search (e.g., `@anthropic/mcp-server-brave-search`) instead of building native backends. This would make web search a configuration concern rather than a code concern. See Section 4.

---

## 4. MCP Integration

*Source: [mcp-rust-integration.md](research/mcp-rust-integration.md) — rmcp crate evaluated.*

### Recommendation: Adopt rmcp as MCP client

**rmcp** (v0.16.0) is the official Rust MCP SDK, actively developed (407+ commits, biweekly releases), and shares our dependency stack (tokio, serde, async-trait). It supports stdio, SSE, and streamable HTTP transports.

```toml
# Cargo.toml
rmcp = { version = "0.16", features = ["client", "transport-child-process", "transport-streamable-http"] }
```

### Transport strategy

| Transport | Use case | Example |
|-----------|----------|---------|
| **Stdio** (default) | Local tool servers spawned per-agent | Filesystem, code analysis, Brave Search |
| **Streamable HTTP** | Shared/remote servers | Centralized web search, databases |
| ~~SSE~~ | Skip — being deprecated in MCP spec | — |

### Dynamic tool registration

MCP tools integrate into the existing `ToolRegistry` via a `McpToolBridge`:

```
Agent startup
  → Read MCP config (per-project or global)
  → Connect to each configured MCP server
  → list_tools() on each server
  → Register as Box<dyn Tool> in ToolRegistry
  → Agent loop uses them like any other tool
```

**Tool name collisions:** Built-in tools win by default. User can configure aliases in `mcp_servers.toml`.

**Lifecycle:** Connections established once at startup, held for agent lifetime, dropped on exit. If an MCP server dies mid-session, tool calls return errors; the agent continues with other tools.

### Configuration

```toml
# .wg/mcp_servers.toml (or per-project)
[servers.brave-search]
command = "npx"
args = ["-y", "@anthropic/mcp-server-brave-search"]
transport = "stdio"
env = { BRAVE_API_KEY = "${BRAVE_API_KEY}" }

[servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/project"]
transport = "stdio"

[servers.shared-search]
url = "http://localhost:8080/mcp"
transport = "streamable-http"
```

### Effort estimate

| Phase | Effort |
|-------|--------|
| Add rmcp dependency | XS (<1hr) |
| MCP config model | S (1-2hr) |
| McpToolBridge (connect, discover, register) | M (3-5hr) |
| Integration with agent loop (async default_all) | S (2-3hr) |
| Tool name collision handling | S (1-2hr) |
| Testing with real MCP server | M (3-4hr) |
| **Total** | **M-L (~12-16hr)** |

### MCP as the web tool strategy

Rather than building native `web_search` and `web_fetch` tools, MCP servers can provide these capabilities:
- `@anthropic/mcp-server-brave-search` for web search
- A simple HTTP-fetch MCP server for web fetch

This makes web tools a **configuration concern** (add an MCP server) rather than a **code concern** (build and maintain backends). The tradeoff is a Node.js dependency for npx-based MCP servers.

---

## 5. Local Model Integration

*Source: [local-model-integration.md](research/local-model-integration.md) — Ollama/vLLM compatibility tested.*

### Compatibility

The existing `OpenAiClient` works with both Ollama and vLLM without code changes. Both expose OpenAI-compatible `/v1/chat/completions` endpoints. The client uses `stream: false`, which is the safest mode for local compatibility.

### Model recommendations

| Tier | Models | VRAM | Quality | Use case |
|------|--------|:----:|:-------:|----------|
| **Production** | Qwen3-235B-A22B, Llama 3.3 70B | 48 GB | Excellent | Replace Claude for most tasks |
| **Simple tasks** | Qwen3-32B | 20 GB (Q4) | Good | File editing, bash commands, structured tasks |
| **Experimentation** | Qwen3-8B | 16 GB | Fair | Simple tasks only |
| **Not viable** | <8B models | — | Poor | Cannot reliably follow tool-use format |

### Key findings

- **Qwen3-32B via Ollama** is the sweet spot for development (good tool calling, single GPU, easy setup)
- **vLLM** is better for multi-agent production (tensor parallelism, continuous batching, guided decoding for more reliable tool arguments)
- **Failure modes** with smaller models: tool name hallucination, malformed JSON arguments, premature completion (emits `stop` instead of `tool_calls`), context window exhaustion (>10 turns)
- **Non-blocking improvements** for local model support: allow empty API key for local servers, expose `timeout_secs` in config, document model-specific `max_tokens` settings

### Configuration

```toml
# Ollama
[native_executor]
provider = "openai"
api_base = "http://localhost:11434"
# Set OPENAI_API_KEY=ollama and WG_MODEL=qwen3:32b

# vLLM
[native_executor]
provider = "openai"
api_base = "http://localhost:8000"
# Launch: vllm serve Qwen/Qwen3-32B --enable-auto-tool-choice --tool-call-parser hermes
```

### Ollama vs vLLM

| Criterion | Ollama | vLLM |
|-----------|--------|------|
| Setup ease | Excellent (single binary) | Moderate (Python, CUDA) |
| Tool reliability | Good (unguided) | Better (guided decoding) |
| Throughput | Single-request | Concurrent batching |
| Best for | Development, single-agent | Multi-agent production |

---

## 6. Protocol Recommendations

### A2A (Agent-to-Agent Protocol)

*Source: [a2a-protocol-applicability.md](research/a2a-protocol-applicability.md)*

**Recommendation: Ignore for now, plan to consume later.**

A2A (v0.3, under Linux Foundation) defines HTTP-based agent-to-agent communication with tasks, messages, artifacts, and streaming. However:

- **Impedance mismatch:** A2A tasks are flat RPCs between two agents. Workgraph tasks are graph nodes with dependencies, cycles, and verification. Wrapping workgraph in A2A would strip away its core value.
- **Ephemeral agents:** Workgraph agents are short-lived subprocesses, not HTTP servers. Implementing A2A would require a persistent HTTP layer.
- **Pre-1.0 spec:** Still evolving rapidly.

**Future path:** When A2A reaches 1.0+ with ecosystem traction, add an `a2a` executor type alongside `claude`/`matrix`/`email`/`shell`. The coordinator would discover an A2A agent via its card, send the task description, poll/stream for completion, and record artifacts — structurally identical to the existing fire-and-forget model.

**State mapping** (for future reference):

| A2A State | Workgraph Status |
|-----------|-----------------|
| submitted/working | InProgress |
| input_required | Blocked (+ HITL notification) |
| completed | Done |
| failed/rejected | Failed |
| canceled | Abandoned |

### MCP (Model Context Protocol)

**Recommendation: Adopt now (as MCP client).**

MCP and A2A are complementary: MCP provides tools/context to a single agent; A2A provides inter-agent communication. Workgraph should consume MCP (for tool extensibility) but not expose agents as MCP servers.

See Section 4 for full integration plan.

### Protocol summary

| Protocol | Relationship | Action | Timeline |
|----------|-------------|--------|----------|
| **MCP** | Tool provider for agents | Adopt rmcp as client | Phase 3 |
| **A2A** | External agent interop | Watch; build `a2a` executor when 1.0 | Future |

---

## 7. Architecture

### Layer model

```
┌─ Coordinator ─────────────────────────────────────────────────────┐
│  Owns: task scheduling, agent lifecycle, stream polling, cost      │
│                                                                    │
│  ┌─ Executor ─────────────────────────────────────────────────┐   │
│  │  Owns: process spawning, wrapper scripts, detachment       │   │
│  │  Interface: spawn() → handle, read_stream(), inject_msg()  │   │
│  │                                                             │   │
│  │  ┌─ Agent Runtime ─────────────────────────────────────┐   │   │
│  │  │  Owns: LLM client, tool-use loop, conversation state│   │   │
│  │  │                                                      │   │   │
│  │  │  ┌─ Tool Layer ─────────────────────────────────┐   │   │   │
│  │  │  │  Sources: built-in, MCP servers, custom      │   │   │   │
│  │  │  └──────────────────────────────────────────────┘   │   │   │
│  │  │                                                      │   │   │
│  │  │  ┌─ LLM Layer ─────────────────────────────────┐   │   │   │
│  │  │  │  Providers: Anthropic, OpenAI-compat, local  │   │   │   │
│  │  │  └──────────────────────────────────────────────┘   │   │   │
│  │  └──────────────────────────────────────────────────────┘   │   │
│  └─────────────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────────────┘
```

The native executor contains all four layers. Claude CLI and Amplifier provide their own Agent Runtime + Tool Layer + LLM Layer, leaving workgraph only the Executor shell.

### Target state

**To make workgraph self-sufficient, the native executor must reach feature parity with Claude Code's agent capabilities.** Claude CLI and Amplifier become optional "premium" executors, not requirements.

| Layer | Claude CLI | Amplifier | Native (current) | Native (target) |
|---|---|---|---|---|
| Model providers | Anthropic only | Any (OpenRouter) | Anthropic + OpenAI-compat | Same |
| Tool count | ~10 | Configurable | 12 (file/bash/wg) | 15+ (add list_dir, web tools via MCP) |
| MCP support | Yes (built-in) | No | No | Yes (via rmcp) |
| Stream output | JSONL (translated) | Bookends | Native StreamEvent | Same |
| Bidirectional | No | No | No | File-based injection |
| Cost tracking | Yes (result event) | Via OpenRouter | Tokens only | Tokens + computed cost |
| External deps | Claude CLI + subscription | Amplifier + OpenRouter | None (Rust binary) | rmcp + optional MCP servers |
| Setup | `npm install -g @anthropic-ai/claude-code` | Bundle scripts | Zero | Zero (MCP servers optional) |

---

## 8. Implementation Roadmap

### Phase 1: Stream Capture — COMPLETE

✅ `src/stream_event.rs` — Unified StreamEvent enum, NDJSON reader/writer
✅ Claude CLI JSONL translation (`translate_claude_event`)
✅ `AgentStreamState` for coordinator-side tracking
✅ Liveness detection via stream staleness
✅ All executors produce `stream.jsonl`

### Phase 2: Native Tool Parity (~400 lines, S-M effort)

**Goal:** Native executor handles 96%+ of tool calls without Bash fallback for common operations.

| Tool | Effort | Notes |
|------|:------:|-------|
| `list_dir` | XS | `fs::read_dir` wrapper |
| `wg_context` | S | Call existing CLI logic |
| `wg_quickstart` | S | Agent bootstrapping |

These are the highest-ROI additions — small effort, high frequency of use.

### Phase 3: MCP Integration (~500 lines, M-L effort)

**Goal:** Native executor can use any MCP tool server, enabling web search, web fetch, and arbitrary tool extensibility without building each tool from scratch.

1. Add `rmcp` dependency with `client` + `transport-child-process` + `transport-streamable-http` features
2. `McpConfig` struct with TOML parsing for `mcp_servers.toml`
3. `McpToolBridge` — connect to servers, discover tools, register in `ToolRegistry`
4. Make `ToolRegistry::default_all()` async for MCP connection startup
5. Tool name collision handling (built-in wins, aliases available)
6. Integration tests with a test MCP server

**This is the most impactful phase.** MCP turns tool extensibility from a code problem into a configuration problem. Users can add web search, database access, or any other capability by pointing at an MCP server.

### Phase 4: Coordinator Stream Integration (~400 lines)

**Goal:** Bidirectional communication. `wg agents` shows live progress.

- Coordinator reads `stream.jsonl` during execution (incremental, offset-based)
- Native executor supports file-based message injection between turns
- Enhanced `wg agents` with Turn, Tool, Tokens, Cost columns
- `wg watch <task>` for live streaming
- Staleness detection: no events for >5min + PID alive → warning

### Phase 5: Self-Sufficient Mode (~200 lines config)

**Goal:** `wg` works out-of-the-box with zero external dependencies beyond a model API key.

- Default executor becomes `native` (instead of `claude`)
- Built-in provider configs for popular endpoints (Anthropic, OpenAI, OpenRouter, Ollama)
- `wg init --provider openai` / `wg init --provider ollama` setup wizards
- Claude CLI and Amplifier become optional "premium" executors
- Documentation for local model setup (Ollama + Qwen3-32B recommended for development)

### Phase 6: A2A Executor (future, when A2A reaches 1.0)

- New `a2a` executor type
- Agent Card discovery and caching
- `sendMessage` dispatch with task description
- SSE/polling for status tracking
- Artifact collection from A2A responses

---

## 9. What to Build Next

**Recommended immediate next step: Phase 3 (MCP Integration).**

Rationale:
1. Phase 2 tools (list_dir, wg_context) are small and can be done alongside or as a quick precursor
2. MCP integration is the **highest-leverage work** — it turns every future tool need into a config change instead of a code change
3. Web search and web fetch (the biggest remaining gaps for research tasks) become free once MCP works
4. The rmcp crate is mature and well-suited to our stack
5. Estimated effort is 12-16 hours — achievable in a focused sprint

**Sequence:**
1. Phase 2 tools (list_dir, wg_context, wg_quickstart) — 1-2 days
2. Phase 3 MCP integration — 2-3 days
3. Phase 4 coordinator stream integration — 2-3 days
4. Phase 5 self-sufficient mode — 1-2 days

Phases 2 and 3 can partially overlap since they touch different parts of the codebase (tools/ vs. a new mcp module).

---

## 10. Build vs. Buy Summary

| Capability | Decision | Rationale |
|---|---|---|
| **LLM client** | Keep ours | Works, tested, no new dependency |
| **Tool-use loop** | Keep ours | Custom logging, wg integration |
| **MCP client** | Adopt rmcp | Official SDK, protocol not framework |
| **Web search** | Via MCP server | Config concern, not code concern |
| **Web fetch** | Via MCP server or build (~100 lines) | Trivial with reqwest |
| **Streaming** | Keep ours (StreamEvent) | Custom event format, already built |
| **Cost tracking** | Build (~200 lines) | Custom integration with price table |
| **Multi-agent coordination** | Keep ours | This IS workgraph |
| **A2A interop** | Defer | Pre-1.0 spec, low priority |

---

## Appendix A: Research Artifacts

| Research | Artifact | Key Finding |
|----------|----------|-------------|
| Stream capture | `src/stream_event.rs` | Implemented. All executors produce unified NDJSON. |
| Tool gap analysis | [native-executor-tool-gaps.md](research/native-executor-tool-gaps.md) | 93.8% coverage already; web tools are the main gap. |
| Local models | [local-model-integration.md](research/local-model-integration.md) | Qwen3-32B+ viable; OpenAiClient works unmodified. |
| Web search APIs | [web-search-api-comparison.md](research/web-search-api-comparison.md) | Serper recommended ($0.30-$1/1k); SearXNG for self-hosted. |
| A2A protocol | [a2a-protocol-applicability.md](research/a2a-protocol-applicability.md) | Ignore now; `a2a` executor when 1.0. |
| MCP Rust SDK | [mcp-rust-integration.md](research/mcp-rust-integration.md) | rmcp v0.16 is production-ready; adopt as client. |

## Appendix B: Ecosystem Links

- [rmcp — Official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk)
- [Rig — Rust LLM framework](https://rig.rs/)
- [Serper — Google Search API](https://serper.dev/)
- [SearXNG — Self-hosted meta-search](https://docs.searxng.org/)
- [Ollama — Local model serving](https://ollama.com/)
- [vLLM — High-throughput model serving](https://docs.vllm.ai/)
