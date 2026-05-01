# TUI Chat Message Interleaving — Investigation Report

**Task:** investigate-tui-chat  
**Date:** 2026-03-27  
**Status:** Complete

---

## Executive Summary

The TUI has **two separate messaging systems** that are relevant here:

1. **Chat system** (`src/chat.rs`) — user↔coordinator communication via inbox/outbox JSONL files. Displayed in the **Chat tab** (panel 0).
2. **Message queue** (`src/messages.rs`) — inter-agent/task messaging via `wg msg send/read`. Displayed in the **Messages tab** (panel 3).

The coordinator agent uses both: it receives user messages via the Chat inbox, and sends messages to agents' tasks via `wg msg send` (executed as a Bash tool call). These `wg msg send` calls appear in the Chat tab as part of the coordinator's `full_response` (inside tool-call boxes like `┌─ Bash ──── │ wg msg send task-id "..." └────`), but they are **appended at the end of the coordinator's complete response**, not interleaved at the temporal point when the target agent actually reads them.

---

## Question 1: Current Behavior — How Does the Chat View Render Messages?

### Chat Tab (Panel 0) — `src/tui/viz_viewer/render.rs:2224` (`draw_chat_tab`)

The Chat tab renders `ChatState.messages: Vec<ChatMessage>` as a linear sequence:

- **User messages** (role=User) — prefixed with `username: `, yellow, warm background
- **Coordinator messages** (role=Coordinator) — prefixed with `↯ `, cyan, rendered with markdown + tool-call boxes
- **System messages** (role=System) — prefixed with `! `, red (errors)

Messages are displayed in **insertion order** — the order they were added to `ChatState.messages`. There is no timestamp-based sorting or interleaving.

For coordinator responses, `full_text` (which includes tool calls) is rendered instead of the summary `text`. This means `wg msg send` commands appear as Bash tool-call boxes **within** the coordinator's response block, but:

- They appear wherever they occurred in the coordinator's output stream
- The coordinator's entire response is a single `ChatMessage` entry — it's atomic
- There is **no concept** of "this message was read by agent X at time T" in the chat view

### Messages Tab (Panel 3) — `src/tui/viz_viewer/render.rs:4417` (`draw_messages_tab`)

The Messages tab shows per-task message queues from `src/messages.rs`. It has:

- `MessageEntry` structs with `direction` (Incoming/Outgoing), `sender`, `body`, `timestamp`, `delivery_status`
- A header showing stats: "3 sent, 2 replies ✓ responded"
- An input area for sending messages to the selected task

This panel shows messages **sorted by their JSONL file order** (which is insertion order, effectively chronological by send time).

### Key Finding

"Sent" messages (from coordinator/user to a task) appear in the Messages tab in send order. There is **no indication** of when the agent actually read/processed the message. In the Chat tab, the coordinator's `wg msg send` calls are embedded in its response but not correlated with the agent's read event.

---

## Question 2: Message Read Timing — When Does an Agent Receive a Message?

### Message Delivery Model

Messages are **pull-based**, not push-based. The flow is:

1. **Send**: `wg msg send task-id "body"` → appends to `.wg/messages/{task-id}.jsonl` with `status: "sent"` and an ISO 8601 timestamp (`src/messages.rs:87-162`)
2. **Read**: Agent runs `wg msg read task-id --agent $WG_AGENT_ID` → returns messages with `id > cursor`, advances cursor, marks messages as `status: "read"` (`src/messages.rs:394-409`)
3. **Acknowledge**: Agent runs `wg msg send task-id "reply..."` — this is a convention (the agent sending a reply is treated as acknowledgment), not a system-level status change

### When Does Reading Happen?

For **both executor types**, message reading is entirely agent-initiated:

- **Claude executor**: The agent (Claude CLI subprocess) decides when to run `wg msg read` via its Bash tool. This happens at whatever point in the agent's conversation the LLM chooses to check messages — typically at the start of a task (per the workflow template) and before marking done.
- **Native executor**: Same — the agent runs `wg msg read` via the Bash tool in the native tool-use loop (`src/executor/native/tools/bash.rs`).

**There is no mid-stream injection.** Messages are not pushed into the agent's context. The agent must actively poll with `wg msg read`. The message sits in the JSONL queue with `status: "sent"` until the agent reads it.

### Timing Metadata Currently Available

- `Message.timestamp` (ISO 8601) — when the message was **sent** (`src/messages.rs:45`)
- `Message.status` — lifecycle: `sent → delivered → read → acknowledged` (`src/messages.rs:14-26`)
- **Cursor files** (`.wg/messages/.cursors/{agent-id}.{task-id}`) — last-read message ID, but **no timestamp of when the read occurred**

### What's Missing

- **No "read_at" timestamp** on messages. We know a message was read (status changes to `read`), but not exactly when.
- **No correlation** between the agent's `wg msg read` execution and the agent's streaming output timeline.

---

## Question 3: Timing Metadata — What Would We Need to Add?

### Currently Stored

| Field | Location | Description |
|-------|----------|-------------|
| `Message.timestamp` | `messages/{task}.jsonl` | Send time (ISO 8601) |
| `Message.status` | `messages/{task}.jsonl` | Lifecycle stage (sent/delivered/read/acknowledged) |
| `Message.id` | `messages/{task}.jsonl` | Monotonic sequence per task |
| Cursor value | `.cursors/{agent}.{task}` | Last-read message ID |

### What Would Need to Be Added

1. **`read_at` field on `Message`** — ISO 8601 timestamp set when `read_unread()` marks the message as `read`. This is straightforward: modify `update_message_statuses()` in `src/messages.rs` to also set a `read_at` field.

2. **`acknowledged_at` field** (optional) — timestamp when status transitions to `acknowledged`.

3. **Stream event correlation** (for TUI rendering) — To interleave messages at the correct point in the agent's output stream, we need to know where in the agent's conversation the `wg msg read` output appeared. Options:
   - **StreamEvent extension**: Add a `MessageRead { task_id, message_ids, timestamp_ms }` variant to `StreamEvent` in `src/stream_event.rs`. The Bash tool executor would emit this when it detects a `wg msg read` command.
   - **Output log parsing**: Parse the agent's output NDJSON log to find the Bash tool call containing `wg msg read` and its position in the turn sequence.

---

## Question 4: Executor Differences

### Claude Executor (CLI Subprocess)

- **Architecture**: Long-lived `claude` CLI process with `--input-format stream-json --output-format stream-json` (`src/commands/service/coordinator_agent.rs:1-11`)
- **Message delivery**: Agent runs `wg msg read` via its Bash tool. The output appears in the Claude CLI's stdout as a tool result.
- **Observability**: The coordinator captures raw JSONL from Claude CLI stdout, translates it via `translate_claude_event()` into `StreamEvent` entries (`src/stream_event.rs:228-322`). Tool calls (including `wg msg read`) appear as `content_block_delta` events in the raw stream.
- **Mid-stream injection**: **Not possible.** Claude CLI does not support injecting messages into an in-flight response. Messages can only be delivered when the agent decides to read them.
- **Timing correlation**: We can parse `raw_stream.jsonl` to find the Bash tool call that ran `wg msg read` and determine its position in the turn sequence. Each tool call has associated `ToolStart`/`ToolEnd` events with `timestamp_ms`.

### Native Executor (Direct API Calls)

- **Architecture**: In-process tool-use loop making API calls to OpenAI-compatible endpoints (`src/executor/native/agent.rs`)
- **Message delivery**: Same as Claude — agent runs `wg msg read` via Bash tool (`src/executor/native/tools/bash.rs`)
- **Observability**: The native agent writes `StreamEvent` entries directly (`src/executor/native/agent.rs` uses `StreamWriter`). Tool calls are recorded in the journal (`src/executor/native/journal.rs`).
- **Mid-stream injection**: **Not possible.** The native executor's tool-use loop is synchronous per turn — it sends a request, gets a response, executes tools, loops. No way to inject mid-response.
- **Timing correlation**: Journal entries include tool calls with timestamps. `ToolStart`/`ToolEnd` stream events are emitted.

### Key Finding

Both executors are **functionally identical** regarding message delivery: messages are pull-based via `wg msg read` Bash tool calls. Neither supports mid-stream message injection. The fundamental mechanism is the same — the LLM agent decides when to check messages.

---

## Question 5: Dynamic Rendering — Could the TUI Insert Messages Inline?

### Current Chat Log Data Structure

```rust
// src/tui/viz_viewer/state.rs:971
pub struct ChatState {
    pub messages: Vec<ChatMessage>,      // Linear message list
    pub editor: EditorState,             // Input area
    pub scroll: usize,                   // Scroll offset (lines from bottom)
    pub awaiting_response: bool,         // Waiting for coordinator
    pub outbox_cursor: u64,              // Last-read outbox message ID
    pub streaming_text: String,          // Partial streaming text
    // ... (other UI state)
}

// src/tui/viz_viewer/state.rs:1055
pub struct ChatMessage {
    pub role: ChatRole,          // User | Coordinator | System
    pub text: String,            // Display text (summary for coordinator)
    pub full_text: Option<String>, // Full response with tool calls
    pub attachments: Vec<String>,
    pub edited: bool,
    pub inbox_id: Option<u64>,
    pub user: Option<String>,
}
```

The chat view is a **flat, append-only list** of `ChatMessage` entries. Each coordinator response is a single entry with `full_text` containing the entire response (text + tool calls). There is no sub-message granularity.

### Can Messages Be Dynamically Inserted?

**Not with the current data structure.** The Chat tab shows user↔coordinator messages. Agent task messages (`wg msg send/read`) are a different system entirely.

For the **Messages tab**, messages are already displayed in chronological order. To show interleaved read events:

1. **Re-rendering approach**: When a message's `status` changes from `sent` to `read`, the Messages tab could insert a visual marker (e.g., "✓ Read by agent at 14:32:05") next to the message. This requires polling the message file for status changes, which the TUI already does (fs watcher triggers `load_messages_panel()`).

2. **Agent output interleaving**: To show messages interleaved with the agent's streaming output (in the Output/Log tab), the output renderer would need to correlate `wg msg read` tool calls with message IDs. The agent's output NDJSON already contains tool calls — we'd need to detect `wg msg read` calls and insert the corresponding message content inline.

---

## Question 6: Feasibility Assessment and Recommended Approach

### Option A: Timestamp-Based Interleaving at Render Time

**Approach**: Add `read_at` timestamp to `Message`. In the Messages tab, sort/display messages with read-time annotations. In the Output tab, parse agent output to find `wg msg read` calls and render message content inline at that point.

**Pros**: Clean separation of concerns, render-time only, no changes to executor behavior  
**Cons**: Requires output log parsing to find `wg msg read` calls, moderate implementation complexity  
**Feasibility**: ✅ High — both executors already log tool calls

### Option B: Explicit "Message Received" Events in Stream

**Approach**: Add a `MessageRead` variant to `StreamEvent`. When the Bash tool detects a `wg msg read` command, emit a `MessageRead` event with the message IDs and content. The TUI can then render these events inline in any view that shows agent activity.

**Pros**: Clean event model, composable across views, explicit correlation  
**Cons**: Requires modifying both executor Bash tool implementations, adds a new stream event type  
**Feasibility**: ✅ High but more invasive — needs changes in `src/executor/native/tools/bash.rs` and Claude CLI output parsing

### Option C: Status-Based Annotations in Messages Tab (Simplest)

**Approach**: Add `read_at` timestamp to `Message`. The Messages tab already shows `delivery_status` — enhance the rendering to show "Read at HH:MM:SS" next to messages. No interleaving with agent output.

**Pros**: Minimal changes, directly addresses the "when was it read" question  
**Cons**: Doesn't actually interleave messages in the agent's output stream  
**Feasibility**: ✅ Very high — small change to `src/messages.rs` + `src/tui/viz_viewer/render.rs`

### Recommended Approach: Option A + C (Phased)

**Phase 1 (Low effort):** Add `read_at` timestamp to `Message` struct and set it in `read_unread()`. Update the Messages tab to display read timestamps. This gives immediate visibility into message delivery timing.

**Phase 2 (Medium effort):** Parse agent output logs to find `wg msg read` Bash tool calls. In the Output/Log tab (and optionally the Chat tab's coordinator response), insert visual markers at the point where the agent read the message. This provides true temporal interleaving.

### Identified Blockers

1. **No `read_at` field** — must be added to `Message` struct in `src/messages.rs` (minor, backward-compatible with `#[serde(default)]`)
2. **Output log parsing** for Phase 2 — need to detect `wg msg read` in Bash tool call inputs across both Claude raw JSONL and native journal formats
3. **Chat tab atomicity** — coordinator responses are single `ChatMessage` entries; breaking them into sub-entries for interleaving is a larger refactor

---

## Source File Inventory

### Core Messaging
| File | Purpose |
|------|---------|
| `src/messages.rs` | Inter-agent message queue (send, read, poll, status tracking) |
| `src/chat.rs` | User↔coordinator chat (inbox/outbox JSONL, streaming) |
| `src/commands/msg.rs` | CLI commands for `wg msg send/read/list/poll` |

### TUI Chat Rendering
| File | Purpose |
|------|---------|
| `src/tui/viz_viewer/state.rs:971` | `ChatState` — chat panel state, message list |
| `src/tui/viz_viewer/state.rs:1055` | `ChatMessage` — display message struct |
| `src/tui/viz_viewer/state.rs:8760` | `poll_chat_messages()` — outbox polling |
| `src/tui/viz_viewer/render.rs:2224` | `draw_chat_tab()` — chat panel renderer |
| `src/tui/viz_viewer/render.rs:2835` | `draw_chat_input()` — input area renderer |

### TUI Messages Panel
| File | Purpose |
|------|---------|
| `src/tui/viz_viewer/state.rs:2367` | `MessagesPanelState` — messages panel state |
| `src/tui/viz_viewer/state.rs:2325` | `MessageDirection` — incoming/outgoing classification |
| `src/tui/viz_viewer/render.rs:4417` | `draw_messages_tab()` — messages panel renderer |

### Executor / Agent
| File | Purpose |
|------|---------|
| `src/commands/service/coordinator_agent.rs` | Coordinator agent (Claude CLI + native loops) |
| `src/executor/native/agent.rs` | Native executor agent loop |
| `src/executor/native/tools/bash.rs` | Native Bash tool (where `wg msg read` runs) |
| `src/stream_event.rs` | Unified stream event format (Init, Turn, ToolStart, ToolEnd, etc.) |
| `src/commands/spawn/execution.rs` | Agent spawning (sets up worktree, context) |

### Storage
| Path | Purpose |
|------|---------|
| `.wg/messages/{task-id}.jsonl` | Per-task message queue |
| `.wg/messages/.cursors/{agent}.{task}` | Read cursor per agent per task |
| `.wg/chat/{coordinator-id}/inbox.jsonl` | User→coordinator messages |
| `.wg/chat/{coordinator-id}/outbox.jsonl` | Coordinator→user responses |
| `.wg/service/agents/{agent-id}/stream.jsonl` | Agent stream events |
| `.wg/service/agents/{agent-id}/raw_stream.jsonl` | Claude CLI raw output |

---

## Message Flow Summary

```
User types in Chat tab
  → TUI sends via IPC to coordinator daemon
    → Coordinator agent receives via stdin injection
      → Coordinator runs `wg msg send <task> "message"` (Bash tool)
        → Message appended to .wg/messages/<task>.jsonl (status: sent)
          → Agent runs `wg msg read <task>` at some future point (Bash tool)
            → Messages returned, cursor advanced, status → read
              → Agent processes message, optionally replies with `wg msg send`

Display in TUI:
  Chat tab:  User msg → [wait] → Coordinator response (contains `wg msg send` in tool boxes)
  Messages tab: Chronological list of messages for selected task (send order)
  Output tab: Agent's streaming output (contains `wg msg read` output inline)
```

The gap: there is no link between "coordinator sent message at T1" and "agent read message at T2" in any TUI view. The recommended approach adds this correlation.
