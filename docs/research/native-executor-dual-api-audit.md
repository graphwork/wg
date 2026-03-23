# Native Executor: Dual API Support Audit

**Task:** research-dual-executor  
**Date:** 2026-03-23  
**Status:** Complete

---

## 1. OpenAI-Compatible Path — Current State

### Completeness: **Mature**

The OpenAI-compatible path lives in `src/executor/native/openai_client.rs` (~950 lines) and is fully functional.

**What exists today:**

| Feature | Status | Notes |
|---------|--------|-------|
| Non-streaming requests | ✅ Complete | `chat_completion()` at line 536 |
| SSE streaming | ✅ Complete | `chat_completion_streaming()` at line 559; auto-enabled for OpenRouter |
| Tool use (function calling) | ✅ Complete | Full translate → execute → translate roundtrip |
| Message format translation | ✅ Complete | `translate_messages()` converts Anthropic-canonical → OpenAI wire format (line 349) |
| Response translation | ✅ Complete | `translate_response()` converts OAI response → canonical `MessagesResponse` (line 467) |
| Tool definition translation | ✅ Complete | `translate_tools()` converts `ToolDefinition` → `OaiFunctionDef` (line 334) |
| Retry with backoff | ✅ Complete | 5 retries for non-streaming, 3 for streaming |
| Usage/token tracking | ✅ Complete | Includes `prompt_tokens_details` (cache read/write) |
| Provider hints | ✅ Complete | OpenRouter gets `HTTP-Referer`, `X-Title`, `cache_control` |
| Streaming tool call accumulation | ✅ Complete | Correctly accumulates partial tool call arguments across chunks |
| API key resolution | ✅ Complete | `OPENROUTER_API_KEY` > `OPENAI_API_KEY` > config file |
| Error handling | ✅ Complete | Structured error parsing, retryable status detection |

**Supported providers via this path:** OpenRouter, OpenAI, Ollama, vLLM, Together, DeepSeek, any OpenAI-compatible endpoint.

**Key implementation detail:** The canonical types are Anthropic-style (`Message`, `ContentBlock`, `ToolUse`, `ToolResult`). The OpenAI client translates to/from these, so the agent loop (`agent.rs`) never sees OpenAI wire format.

---

## 2. Anthropic-Compatible Path — Current State

### Completeness: **Mature**

The Anthropic path lives in `src/executor/native/client.rs` (~636 lines) and is fully functional.

**What exists today:**

| Feature | Status | Notes |
|---------|--------|-------|
| Non-streaming requests | ✅ Complete | `messages()` at line 235 |
| SSE streaming | ✅ Complete | `messages_streaming()` at line 254; full SSE parser |
| Tool use | ✅ Complete | Native format — no translation needed |
| Retry with backoff | ✅ Complete | 5 retries, exponential backoff up to 60s |
| Usage/token tracking | ✅ Complete | Includes `cache_creation_input_tokens`, `cache_read_input_tokens` |
| API key resolution | ✅ Complete | `ANTHROPIC_API_KEY` > config `[native_executor]` > `~/.config/anthropic/api_key` |
| Error handling | ✅ Complete | Structured `ApiErrorResponse` parsing |
| Stream assembly | ✅ Complete | `assemble_stream_response()` handles all event types |
| Base URL override | ✅ Complete | For proxies and testing |

**Important:** This is a *native* Anthropic API client, **separate from the `claude` executor** which shells out to the Claude CLI. The `claude` executor type (the default) runs `claude --print --verbose ...` as a subprocess. The native Anthropic client runs HTTP calls in-process.

**Both API paths implement the `Provider` trait** (defined in `provider.rs`, line 24):
```rust
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn model(&self) -> &str;
    fn max_tokens(&self) -> u32;
    async fn send(&self, request: &MessagesRequest) -> Result<MessagesResponse>;
}
```

---

## 3. Conversation Capture — Current State

### What gets logged/captured:

The native executor's `AgentLoop` (`agent.rs`) logs to an NDJSON file (`agent.ndjson`) at the path `<workgraph_dir>/agents/<agent-id>/agent.ndjson`.

**Log event types** (line 52–71):

| Event Type | Content Captured | Full Data? |
|------------|-----------------|------------|
| `Turn` | turn number, role ("assistant"), content block summaries, usage | **Partial** — text included, tool_use only captures id+name (not input), tool_result only captures id+is_error (not content) |
| `ToolCall` | tool name, full input JSON, full output string, is_error flag | **Full** |
| `Result` | final_text, total turns, total usage | **Summary** |

**What is NOT persisted:**
- **Full conversation history** (system prompt + all messages) — the `messages: Vec<Message>` vector in `AgentLoop::run()` (line 147) is in-memory only and discarded on completion.
- **User messages** (tool results sent back to the API) — not logged as separate events.
- **Raw API request/response bodies** — not captured.
- **System prompt** — not logged (only passed to each API request).

**Stream events** are also written to `<agent-dir>/stream.jsonl` (via `StreamWriter`) with:
- `init` event (executor type, model)
- `turn` events (turn number, tool names, per-turn usage)
- `tool_start`/`tool_end` events (name, error status, duration)
- `result` event (success, total usage, model)

**Gap:** The full message history (the actual conversation sent to and received from the LLM) is **not persisted**. To replay or audit a conversation, you would need to reconstruct it from the NDJSON log, which is lossy (tool use inputs are omitted from Turn events, though captured separately in ToolCall events).

---

## 4. Tool Protocol Differences — How Currently Handled

### Architecture: **Translation layer in `OpenAiClient`, canonical types are Anthropic-native**

The canonical types are defined in `client.rs` and follow Anthropic's Messages API format:

```
ContentBlock::ToolUse { id, name, input }     → Anthropic native
ContentBlock::ToolResult { tool_use_id, content, is_error } → Anthropic native
ToolDefinition { name, description, input_schema } → Anthropic native
```

The OpenAI client translates **both directions**:

**Request translation** (`translate_messages`, `translate_tools`):
- `ToolDefinition` → `OaiToolDef { type: "function", function: { name, description, parameters } }`
- `ContentBlock::ToolUse` → `OaiToolCall { id, type: "function", function: { name, arguments: JSON string } }`
- `ContentBlock::ToolResult` → `OaiMessage { role: "tool", content, tool_call_id }`
- System message: Anthropic uses a separate `system` field; OpenAI uses a `role: "system"` message

**Response translation** (`translate_response`):
- `OaiToolCall` → `ContentBlock::ToolUse` (arguments: JSON string → parsed `serde_json::Value`)
- `finish_reason: "stop"` → `StopReason::EndTurn`
- `finish_reason: "tool_calls"` → `StopReason::ToolUse`
- `finish_reason: "length"` → `StopReason::MaxTokens`
- `OaiUsage` → `Usage` (with `prompt_tokens_details.cached_tokens` mapped to `cache_read_input_tokens`)

**Assessment:** There is a clean abstraction layer. The `Provider` trait insulates the agent loop from wire format differences. Adding a new provider would only require implementing `Provider::send()` with the appropriate translation. No hardcoding — the design is provider-agnostic by default.

---

## 5. Config Surface — How Users Select API Backend

### Provider Selection Hierarchy

Provider routing happens in `provider.rs:create_provider_ext()` (line 58):

```
provider_override (CLI --provider)
  → config [native_executor].provider
    → WG_LLM_PROVIDER env var
      → model string heuristic: contains "/" → "openai", else → "anthropic"
```

### Executor Type Selection

The coordinator selects the executor type (not the same as API provider) via `coordinator.rs:2679`:

```
task.exec_mode == "shell" or task.exec is set → "shell"
  → agent.executor (from agency identity)
    → config [coordinator].executor (default: "claude")
```

Supported executor types: `claude` (CLI subprocess), `amplifier`, `native` (in-process), `shell`.

### Per-Task Configuration

Tasks can set:
- `task.model` — model override
- `task.provider` — provider override (e.g., "openai", "anthropic")
- `task.endpoint` — named endpoint override
- `task.exec_mode` — bare/light/full/shell

### Endpoint Configuration

`config.toml` supports named endpoints:

```toml
[[llm_endpoints.endpoints]]
name = "my-openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
is_default = true
```

The endpoint system (`EndpointConfig` in `config.rs:300`) supports:
- `api_key` (inline), `api_key_file` (path), `api_key_env` (env var name)
- Automatic env var fallback by provider type
- Default URL per provider (`anthropic` → api.anthropic.com, `openai` → api.openai.com/v1, etc.)

### Native Executor Selection

To use the native executor (in-process LLM calls), set:
```toml
[coordinator]
executor = "native"
```

Or per-agent via agency: `agent.executor = "native"`.

The native executor routes to Anthropic or OpenAI based on the model string or explicit provider setting — **there is no separate config to choose "use Anthropic API" vs "use OpenAI API" within the native executor**. It's automatic.

---

## Gap Analysis: What's Missing for Full Dual-API Parity

### Already at Parity ✅

| Capability | Anthropic Path | OpenAI Path |
|-----------|---------------|-------------|
| Non-streaming requests | ✅ | ✅ |
| SSE streaming | ✅ | ✅ |
| Tool use roundtrip | ✅ | ✅ |
| Retry with backoff | ✅ | ✅ |
| Usage/token tracking | ✅ | ✅ |
| API key resolution | ✅ | ✅ |
| Error handling | ✅ | ✅ |
| Provider trait impl | ✅ | ✅ |
| Base URL override | ✅ | ✅ |
| Cache token tracking | ✅ | ✅ |

### Gaps Per Path

#### Anthropic Path — Missing

| Feature | Status | Priority |
|---------|--------|----------|
| Streaming not used by agent loop | ⚠️ Implemented but unused | Low — `AgentLoop` always sends `stream: false` (line 177 of `agent.rs`) |
| Extended thinking / beta headers | ❌ Missing | Medium — needed for Claude models with `thinking` capability |
| Prompt caching (`cache_control` blocks) | ❌ Missing | Medium — Anthropic supports explicit `cache_control` markers; the client doesn't set them |
| Image/PDF content blocks | ❌ Missing | Low — `ContentBlock` only has Text, ToolUse, ToolResult |
| Batch API | ❌ Missing | Low |

#### OpenAI Path — Missing

| Feature | Status | Priority |
|---------|--------|----------|
| Structured outputs (response_format) | ❌ Missing | Low |
| Parallel tool calls control | ❌ Missing | Low — OpenAI has `parallel_tool_calls` parameter |
| Image/vision messages | ❌ Missing | Low |
| `tool_choice` parameter | ❌ Missing | Low |

#### Shared / Cross-Cutting Gaps

| Feature | Status | Priority | Impact |
|---------|--------|----------|--------|
| **Full conversation persistence** | ❌ Missing | **High** | Cannot replay, audit, or resume conversations. The downstream `design-conversation-layer` task depends on this. |
| **Streaming in agent loop** | ❌ Not wired | Medium | `AgentLoop` always uses non-streaming `Provider::send()`. Both providers support streaming but the loop doesn't use it. Real-time token output would improve observability. |
| **Cost tracking** | ❌ Missing | Medium | `TotalUsage.cost_usd` is always `None` (line 385 of `agent.rs`). No price table exists. |
| **Token counting / context window management** | ❌ Missing | Medium | No pre-flight token count check. Agent loop has no way to know when it's approaching the context limit. |
| **Multi-turn resume** | ❌ Missing | Medium | If an agent is interrupted, the conversation is lost. No checkpoint/resume for native executor (only `claude` executor has `--resume` support via session_id). |
| **System prompt caching** | ⚠️ Partial | Medium | OpenRouter path sends `cache_control: ephemeral`. Anthropic path doesn't set any cache markers. System prompt is re-sent every turn without caching. |

---

## Recommended Architecture: Unified Conversation Layer

### Current Architecture (Good Foundation)

```
AgentLoop (agent.rs)
    ↓ uses canonical types from client.rs
Provider trait (provider.rs)
    ├── AnthropicClient (client.rs) — implements Provider
    └── OpenAiClient (openai_client.rs) — implements Provider, translates wire format
```

This is already well-factored. The `Provider` trait is clean and the translation happens at the right boundary.

### Proposed: Conversation Layer (New)

```
AgentLoop (agent.rs)
    ↓
ConversationManager (NEW)
    ├── Persists full message history to disk (NDJSON or structured JSON)
    ├── Manages context window budget (token counting)
    ├── Handles conversation compression/summarization when approaching limits
    ├── Supports checkpoint/resume
    └── Emits conversation events for observability
    ↓
Provider trait (provider.rs) — unchanged
    ├── AnthropicClient
    └── OpenAiClient
```

### Key Design Decisions

1. **Persistence format:** NDJSON file per conversation at `<agent-dir>/conversation.jsonl`. Each line is a `Message` (the canonical type). This gives:
   - Full replay capability
   - Incremental append (no rewrite)
   - Git-friendly (one line per message)
   - Compatible with the existing NDJSON log pattern

2. **Conversation state struct:**
   ```rust
   pub struct Conversation {
       pub system_prompt: String,
       pub messages: Vec<Message>,
       pub total_usage: Usage,
       pub persistence_path: PathBuf,
   }
   ```

3. **Token budget management:** The conversation layer should:
   - Count tokens before each `Provider::send()` call
   - Trigger conversation compression when approaching the model's context window
   - Use the existing `ModelRegistry` to look up context window sizes
   - Compression strategy: summarize early turns, keep recent turns verbatim

4. **Resume support:** On agent restart:
   - Load `conversation.jsonl` to reconstruct message history
   - Resume the agent loop from where it left off
   - The `AgentLoop` would accept an optional pre-loaded conversation

5. **Streaming integration:** The conversation layer is the right place to wire streaming:
   - Add `Provider::send_streaming()` that yields events
   - ConversationManager can flush partial text to the persistence file
   - StreamWriter integration moves into the conversation layer

### Implementation Sequence

1. **`ConversationManager` struct** — persistence + message tracking (no token counting yet)
2. **Wire into `AgentLoop`** — replace the bare `Vec<Message>` with `ConversationManager`
3. **Token counting** — add pre-flight budget checks using `ModelRegistry`
4. **Resume** — load conversation from disk on restart
5. **Streaming** — add `Provider::send_streaming()` and wire through the manager

Each step is independently shippable and testable.

---

## File Reference Index

| File | Role | Key Lines |
|------|------|-----------|
| `src/executor/native/mod.rs` | Module declaration | 1–17 |
| `src/executor/native/client.rs` | Anthropic client + canonical types | Types: 18–96, Client: 187–383, Provider impl: 386–402, SSE parser: 523–563 |
| `src/executor/native/openai_client.rs` | OpenAI-compatible client + translation | Types: 22–149, Streaming types: 151–204, Client: 211–868, Translation: 334–533 |
| `src/executor/native/provider.rs` | Provider trait + routing | Trait: 24–39, Router: 42–187 |
| `src/executor/native/agent.rs` | Agent loop + logging | Loop: 141–337, Log types: 52–100, Stream events: 370–401 |
| `src/executor/native/bundle.rs` | Bundle system (tool filtering) | Bundle struct: 16–30, Resolution: 131–170 |
| `src/executor/native/tools/mod.rs` | Tool registry + dispatch | Registry: 55–141, Tool trait: 43–52 |
| `src/commands/native_exec.rs` | CLI entry point for native executor | Full file, 145 lines |
| `src/commands/spawn/execution.rs` | Spawn logic + command building | Native executor: 767–808, Provider resolution: 233–270 |
| `src/service/executor.rs` | Executor config system | ExecutorConfig: 666–774, Registry: 777–900 |
| `src/config.rs` | Project config | EndpointConfig: 300–456, CoordinatorConfig: 1690+, AgentConfig: 1660–1687 |

