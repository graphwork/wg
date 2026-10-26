# Investigation: Native Executor Not Streaming Tool Output to TUI

## Bug Summary

When using `--executor native --model minimax/minimax-m2.5` (OpenRouter), the TUI doesn't show:
1. Command outputs in the log view in a streaming way
2. Tool call invocations in the chat view (even though tools DO execute correctly)

## Root Causes

There are **two distinct root causes**, one for each symptom.

---

### Root Cause 1: Coordinator chat view — no incremental streaming

**Symptom**: The coordinator replies, but text and tool calls appear all at once instead of streaming progressively.

**Mechanism**: The native coordinator loop (`native_coordinator_loop()` in `src/commands/service/coordinator_agent.rs:1424`) calls `client.send(&api_request)` which **blocks until the full response is assembled**, then processes all content blocks in a burst.

Even though the OpenAI client (`src/executor/native/openai_client.rs`) does SSE streaming internally for OpenRouter (enabled at line 321 via `with_provider_hint("openrouter")`), the `chat_completion_streaming()` method (line 604) **accumulates all chunks internally** and returns a single complete `MessagesResponse`. The streaming happens inside the HTTP client but is invisible to the caller.

The `Provider::send()` trait method (line 914) returns `Result<MessagesResponse>` — a batch response with no streaming callback. There is no mechanism for the coordinator to receive incremental chunks.

**Compare with Claude CLI path**: The Claude CLI executor spawns a child process that emits JSONL events to stdout one at a time. The `stdout_reader()` function (`coordinator_agent.rs:937`) processes each line as it arrives and sends `ResponseEvent::Text`, `ResponseEvent::ToolUse`, `ResponseEvent::ToolResult` through a channel. The `collect_response_streaming()` function then writes each event to the `.streaming` file immediately, giving the TUI progressive updates.

**Key files**:
- `src/executor/native/provider.rs:38` — `Provider::send()` trait: returns batch response, no streaming callback
- `src/executor/native/openai_client.rs:604-650` — `chat_completion_streaming()`: SSE → batch response internally
- `src/executor/native/client.rs:267-269` — `messages_streaming()`: same pattern for Anthropic
- `src/commands/service/coordinator_agent.rs:1605-1771` — native coordinator loop: processes all blocks at once after `send()` returns
- `src/commands/service/coordinator_agent.rs:1667-1694` — streaming writes happen, but only in a burst after the API call

**Why the TUI gets nothing during the API call**: While `client.send()` is blocking (potentially for many seconds), the `streaming_text` variable is empty and no `chat::write_streaming()` calls happen. The TUI polls `read_streaming()` and sees empty content. Once `send()` returns, all content is written in rapid succession, appearing as a single dump.

---

### Root Cause 2: Agent log/output views — format mismatch + wrong file

**Symptom**: Tool call invocations don't appear in the agent's log/output/detail views.

**Mechanism (two sub-issues)**:

#### 2a. output.log contains only stderr, not structured events

The native executor worker agent (`wg native-exec`, `src/commands/native_exec.rs`) writes its structured NDJSON log to `agents/{id}/agent.ndjson` (line 80), NOT to `output.log`. The spawn wrapper script (`src/commands/spawn/execution.rs:908`) redirects stdout+stderr to `output.log`, but the native executor only writes `eprintln!` debug messages to stderr. So `output.log` contains messages like:
```
[native-exec] Starting agent loop for task 'foo' with model 'minimax/minimax-m2.5'...
[openai-client] Stream complete: 42 chunks, 1234 text chars, 3 tool calls
```
...not structured JSONL.

#### 2b. TUI parsers expect Claude CLI JSONL format

The TUI's `update_agent_streams()` (`src/tui/viz_viewer/state.rs:7467`) reads `output.log` and expects Claude CLI format:
- `"type": "assistant"` with `message.content` arrays
- `"type": "tool_use"` (top-level)
- `"type": "tool_result"`
- `"type": "result"`

The native executor's log format uses completely different event types:
- `"type": "turn"` with `turn`, `role`, `content` (not nested under `message`)
- `"type": "tool_call"` with `name`, `input`, `output`, `is_error`
- `"type": "result"` with `final_text`, `turns`, `total_usage` (different fields)

Same applies to `extract_enriched_text_from_log()` (line 11235) and `update_output_pane()` (line 7631).

**Key files**:
- `src/commands/native_exec.rs:76-83` — writes to `agent.ndjson`, not `output.log`
- `src/commands/spawn/execution.rs:906-912` — native wrapper only captures stderr to `output.log`
- `src/executor/native/agent.rs:60-81` — `LogEvent` enum: `turn`, `tool_call`, `result` format
- `src/tui/viz_viewer/state.rs:7467-7625` — `update_agent_streams()`: expects Claude CLI `"assistant"` format
- `src/tui/viz_viewer/state.rs:11235-11366` — `extract_enriched_text_from_log()`: expects `"assistant"` format

---

## Proposed Fixes

### Fix 1: Incremental streaming for native coordinator (chat view)

**Approach**: Add a streaming callback mechanism to the `Provider` trait so the native coordinator can receive chunks as they arrive.

**Option A (recommended — minimal change)**: Add an optional `send_streaming()` method to `Provider` that takes a callback/channel for incremental events:

```rust
// In provider.rs
#[async_trait]
pub trait Provider: Send + Sync {
    // ... existing methods ...

    /// Send a streaming completion request, calling the callback for each chunk.
    /// Default implementation falls back to non-streaming send().
    async fn send_streaming(
        &self,
        request: &MessagesRequest,
        on_event: &dyn Fn(StreamChunk) + Send + Sync,
    ) -> Result<MessagesResponse> {
        self.send(request).await  // default: no incremental streaming
    }
}
```

For the OpenAI client, implement this by forwarding SSE chunks to the callback during `streaming_attempt()` (around line 695 where chunks are processed). For the Anthropic client, similarly forward `content_block_delta` events.

The native coordinator loop (`native_coordinator_loop`) would then call `send_streaming()` with a callback that writes to `chat::write_streaming()` incrementally, mirroring what the Claude CLI path does via `ResponseEvent`.

**Option B (simpler but less clean)**: Instead of modifying the `Provider` trait, have the native coordinator loop spawn a background task that reads `stream.jsonl` and translates events to streaming text (similar to how the Claude CLI path's `stdout_reader` works). This would be a polling approach rather than a push approach.

### Fix 2: Agent log/output view format compatibility

**Option A (recommended — dual-format parser)**: Update the TUI's `update_agent_streams()` and `extract_enriched_text_from_log()` to recognize both Claude CLI format AND native executor format:

```rust
// In update_agent_streams(), add handling for native format:
match msg_type {
    "assistant" => { /* existing Claude CLI handling */ }
    "turn" => {
        // Native executor format
        info.message_count += 1;
        if let Some(content) = val.get("content").and_then(|c| c.as_array()) {
            for block in content {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => { /* extract text snippet */ }
                    Some("tool_use") => { /* extract tool name + input */ }
                    _ => {}
                }
            }
        }
    }
    "tool_call" => {
        // Native executor tool call log
        let name = val.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        info.latest_snippet = Some(name.to_string());
        info.latest_is_tool = true;
    }
    // ... existing handlers ...
}
```

**Also**: Read from `agent.ndjson` as a fallback when `output.log` doesn't contain JSONL events. This can be detected by checking if the first line of `output.log` starts with `{` (JSON) or `[` (debug text).

**Option B (alternative)**: Make the native executor write Claude CLI compatible JSONL to stdout (which gets redirected to `output.log` by the wrapper). This means changing `LogEvent` format to match Claude CLI format, or adding a separate stdout writer alongside the `agent.ndjson` writer.

---

## Files That Need Modification

### For Fix 1 (coordinator streaming):
1. `src/executor/native/provider.rs` — Add streaming method to `Provider` trait
2. `src/executor/native/openai_client.rs` — Implement streaming with callback in `streaming_attempt()`
3. `src/executor/native/client.rs` — Implement streaming with callback for Anthropic SSE
4. `src/commands/service/coordinator_agent.rs` — Update `native_coordinator_loop()` to use streaming send

### For Fix 2 (agent output views):
1. `src/tui/viz_viewer/state.rs` — Update `update_agent_streams()`, `extract_enriched_text_from_log()`, and `update_output_pane()` to handle native executor format
2. `src/commands/native_exec.rs` — Optionally also write Claude CLI compatible JSONL to stdout

### Optional (both fixes benefit):
- `src/executor/native/agent.rs` — The worker agent loop could also benefit from streaming callbacks for `stream.jsonl` events to be more granular
