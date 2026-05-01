# Unified Conversation Layer: Architecture & Specification

**Task:** design-conversation-layer  
**Date:** 2026-03-23  
**Status:** Complete  
**Depends on:** research-dual-executor (native-executor-dual-api-audit.md)

---

## Table of Contents

1. [Component Architecture](#1-component-architecture)
2. [Internal Message Representation](#2-internal-message-representation)
3. [Journal File Format](#3-journal-file-format)
4. [Resume Protocol](#4-resume-protocol)
5. [Tool Abstraction](#5-tool-abstraction)
6. [Migration Path](#6-migration-path)
7. [Worked Example](#7-worked-example)

---

## 1. Component Architecture

### Current Architecture

```
AgentLoop (agent.rs)
    │ owns Vec<Message> (in-memory, discarded on completion)
    │ owns LogEvent writer (lossy NDJSON summary)
    │ owns StreamWriter (observability events)
    ↓
Provider trait (provider.rs)
    ├── AnthropicClient (client.rs)     ─ native Anthropic wire format
    └── OpenAiClient (openai_client.rs) ─ translates canonical ↔ OpenAI wire format
```

**Problems:**
- Full conversation history is in-memory only — lost on crash or completion
- LogEvent is lossy (tool_use inputs omitted from Turn events)
- No resume capability
- No context window management
- Agent loop is tightly coupled to the provider

### Proposed Architecture

```
AgentLoop (agent.rs)
    │
    ↓
ConversationManager (NEW: conversation.rs)
    ├── Journal (NEW: journal.rs)
    │     └── Appends JournalEntry records to .wg/output/<task-id>/conversation.jsonl
    ├── ContextBudget (NEW: context_budget.rs)
    │     └── Tracks token usage, triggers compaction when approaching limits
    └── ResumeLoader (NEW: resume.rs)
          └── Loads journal → reconstructs messages Vec + metadata
    ↓
Provider trait (provider.rs) ── UNCHANGED
    ├── AnthropicClient
    └── OpenAiClient
```

### Component Responsibilities

| Component | Responsibility | Owns |
|-----------|---------------|------|
| `AgentLoop` | Orchestrates turns, executes tools, decides when to stop | Turn counter, tool registry, stop conditions |
| `ConversationManager` | Mediates between AgentLoop and Provider; journals every exchange | Journal, ContextBudget, system prompt, messages Vec |
| `Journal` | Append-only persistence of conversation entries | File handle, flush policy |
| `ContextBudget` | Token counting and compaction decisions | Token counts, model limits, compaction state |
| `ResumeLoader` | Reconstructs a ConversationManager from a journal file | Nothing persistent (stateless transformer) |
| `Provider` | Sends requests, receives responses, translates wire formats | HTTP client, auth credentials |

### Key Design Principle: The ConversationManager Owns the Messages

Today, `AgentLoop::run()` owns `messages: Vec<Message>` directly (agent.rs:147). In the new design, `ConversationManager` owns the messages vector and the `AgentLoop` interacts with it through a small API:

```rust
impl ConversationManager {
    /// Start a new conversation with an initial user message.
    fn new(config: ConversationConfig) -> Result<Self>;

    /// Resume from an existing journal file.
    fn resume(journal_path: &Path, config: ConversationConfig) -> Result<Self>;

    /// Add a user message (initial prompt or tool results).
    fn push_user(&mut self, content: Vec<ContentBlock>);

    /// Send the current conversation to the provider and record the response.
    /// Returns the assistant's response for the AgentLoop to process.
    async fn send(&mut self, provider: &dyn Provider) -> Result<MessagesResponse>;

    /// Get the full messages history (for inspection/debugging).
    fn messages(&self) -> &[Message];

    /// Get accumulated usage.
    fn total_usage(&self) -> &Usage;
}
```

This is a **thin wrapper** — it does not change the control flow of the agent loop. The loop still decides when to send, what tool results to push, and when to stop. The ConversationManager just ensures everything gets journaled and the context budget is respected.

---

## 2. Internal Message Representation

### Current Canonical Types (RETAINED)

The existing types in `client.rs` are already provider-agnostic and well-designed. They serve as the internal representation. No new message types are needed.

```rust
// client.rs — these are the canonical types, unchanged

pub enum Role { User, Assistant }

pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
}

pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: Option<u32>,
    pub cache_read_input_tokens: Option<u32>,
}
```

**Rationale:** The research audit confirmed these types already serve as the lingua franca — `OpenAiClient` translates to/from them, `AnthropicClient` uses them natively. Adding a third representation would create unnecessary translation layers.

### Extension: Metadata Envelope

The journal needs metadata that the bare `Message` type doesn't carry. Rather than modifying `Message`, we wrap it:

```rust
/// A single entry in the conversation journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Monotonically increasing sequence number within this conversation.
    pub seq: u64,

    /// ISO-8601 timestamp of when this entry was recorded.
    pub timestamp: String,

    /// The kind of entry.
    #[serde(flatten)]
    pub kind: JournalEntryKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "entry_type", rename_all = "snake_case")]
pub enum JournalEntryKind {
    /// Conversation metadata — first entry in every journal.
    Init {
        model: String,
        provider: String,
        system_prompt: String,
        /// Tool definitions available in this conversation.
        tools: Vec<ToolDefinition>,
        /// Task ID if running within workgraph.
        task_id: Option<String>,
    },

    /// A message in the conversation (user or assistant).
    Message {
        role: Role,
        content: Vec<ContentBlock>,
        /// Usage stats (present only for assistant messages, from the API response).
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        /// API response ID (present only for assistant messages).
        #[serde(skip_serializing_if = "Option::is_none")]
        response_id: Option<String>,
        /// Stop reason (present only for assistant messages).
        #[serde(skip_serializing_if = "Option::is_none")]
        stop_reason: Option<StopReason>,
    },

    /// A tool execution record (between the assistant's tool_use and the user's tool_result).
    ToolExecution {
        /// Matches the tool_use id in the preceding assistant message.
        tool_use_id: String,
        name: String,
        input: serde_json::Value,
        output: String,
        is_error: bool,
        /// Wall-clock duration in milliseconds.
        duration_ms: u64,
    },

    /// Compaction marker — indicates that messages before this point were summarized.
    Compaction {
        /// Sequence number of the last entry that was compacted.
        compacted_through_seq: u64,
        /// The summary that replaces the compacted messages.
        summary: String,
        /// Number of original messages that were compacted.
        original_message_count: u32,
        /// Total tokens in the compacted region (for budget accounting).
        original_token_count: u32,
    },

    /// Conversation ended.
    End {
        reason: EndReason,
        total_usage: Usage,
        turns: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// Agent produced a final text response.
    Complete,
    /// Hit max turns limit.
    MaxTurns,
    /// Agent was interrupted/crashed (written on resume, not at crash time).
    Interrupted,
    /// Error during execution.
    Error { message: String },
}
```

### Why This Shape

1. **`Init` entry**: Captures everything needed to understand or replay the conversation without external context. The system prompt and tool definitions are part of the conversation — they must be journaled.

2. **`Message` entry**: Directly contains the canonical `Role` + `Vec<ContentBlock>`. No translation needed. The `usage`, `response_id`, and `stop_reason` fields are only populated for assistant messages (they come from `MessagesResponse`).

3. **`ToolExecution` entry**: Sits between the assistant's `tool_use` request and the user's `tool_result` response. This is intentionally separate from `Message` because tool executions are side effects, not conversation turns. They capture timing and the full input/output (fixing the current lossy logging).

4. **`Compaction` entry**: A tombstone that says "everything before seq N was replaced by this summary." This is the key to the resume protocol's compaction strategy (§4).

5. **`End` entry**: Clean termination marker. Not written on crash (that's the point — its absence signals an incomplete conversation).

---

## 3. Journal File Format

### File Location

```
.wg/output/<task-id>/conversation.jsonl
```

This follows the existing pattern where agent output goes to `.wg/output/<task-id>/` (currently `agent.ndjson` and `stream.jsonl` live at `.wg/agents/<agent-id>/`). Using the task-id directory means:
- Multiple agent runs for the same task (retries, resumes) append to the same journal
- The journal survives agent identity changes (new agent picking up a failed task)
- Output is colocated with other task artifacts

### Format Rules

1. **One JSON object per line** (JSONL/NDJSON). No pretty-printing.
2. **Append-only**. Never rewrite or truncate the file.
3. **Flush after every entry**. Each `JournalEntry` is followed by `\n` and an `fsync` (or at minimum `flush`). This ensures crash safety — entries up to the last flush are guaranteed on disk.
4. **UTF-8 encoded**. No BOM.
5. **No external references**. The journal is self-contained. Tool definitions, system prompt, and all message content are inline.

### Write Protocol

```rust
pub struct Journal {
    file: std::fs::File,
    seq: u64,
}

impl Journal {
    /// Create or open (for append) a journal file.
    pub fn open(path: &Path) -> Result<Self>;

    /// Append an entry, auto-assigning seq and timestamp.
    pub fn append(&mut self, kind: JournalEntryKind) -> Result<()>;

    /// Read all entries from a journal file (for resume).
    pub fn read_all(path: &Path) -> Result<Vec<JournalEntry>>;
}
```

The `append` method:
1. Increments `self.seq`
2. Captures `Utc::now()` as ISO-8601
3. Serializes `JournalEntry { seq, timestamp, kind }` as a single JSON line
4. Writes the line + `\n` to the file
5. Calls `file.flush()` (and optionally `file.sync_data()` for durability)

### Ordering Guarantees

Within a single conversation turn, entries are written in this order:

```
Message { role: User, ... }          ← user prompt or tool results
Message { role: Assistant, ... }     ← API response
ToolExecution { ... }                ← for each tool_use in the response (in order)
ToolExecution { ... }
Message { role: User, ... }          ← tool results sent back
... (next turn)
```

This ordering means:
- A `ToolExecution` always appears between the assistant message that requested it and the user message that delivers the result
- On crash, the last complete entry tells you exactly where the conversation was

### Compatibility

The journal format is **versioned implicitly** by the `entry_type` discriminator. New entry types can be added without breaking old readers (they skip unknown types). Fields within entry types use `#[serde(default)]` where appropriate so that missing fields in old journals don't cause parse errors.

---

## 4. Resume Protocol

### Overview

Resume allows a new agent process to pick up a conversation from where a previous agent left off (or crashed). This is critical for:
- **Crash recovery**: Agent OOM'd, timed out, or was killed
- **Task retry**: Previous attempt failed, coordinator dispatches a new agent
- **Context window exhaustion**: Conversation grew too large, needs compaction and restart

### Step 1: Load Journal

```rust
let entries = Journal::read_all(&journal_path)?;
```

Parse every line. Malformed lines (from a crash mid-write) are skipped with a warning — they represent incomplete writes.

### Step 2: Reconstruct State

The `ResumeLoader` walks the entries and reconstructs:

```rust
pub struct ResumedConversation {
    /// The system prompt from the Init entry.
    pub system_prompt: String,
    /// Tool definitions from the Init entry.
    pub tools: Vec<ToolDefinition>,
    /// The reconstructed messages array (ready to send to the API).
    pub messages: Vec<Message>,
    /// Accumulated usage from all previous turns.
    pub total_usage: Usage,
    /// The turn count (number of assistant Message entries).
    pub turns: u32,
    /// The last seq number (for continuing the journal).
    pub last_seq: u64,
    /// Whether the conversation ended cleanly (End entry present).
    pub is_complete: bool,
    /// Model and provider from Init.
    pub model: String,
    pub provider: String,
}
```

Reconstruction algorithm:

```
for each entry in entries:
    match entry.kind:
        Init { .. }       → store system_prompt, tools, model, provider
        Message { .. }    → append to messages Vec; if assistant, add usage, increment turns
        ToolExecution { } → skip (informational; the tool results are in the subsequent Message)
        Compaction { .. } → discard all messages up to compacted_through_seq,
                            prepend summary as a User message
        End { .. }        → set is_complete = true
```

### Step 3: Handle Stale State

When a conversation is resumed, the filesystem may have changed since the last turn. Tool results from previous turns may reference files that no longer exist, contain outdated content, or describe state that is no longer accurate.

**Strategy: Don't re-validate old tool results.**

Old tool results are part of the conversation history — they reflect what the agent saw at the time. Re-executing them would:
- Violate the conversation's internal consistency (the assistant's subsequent reasoning was based on the original results)
- Be expensive and potentially dangerous (re-running bash commands)
- Be unnecessary if the resumed agent is going to make new tool calls anyway

Instead, the resume protocol injects a **stale-state notice** as the first new user message:

```
[Resume notice] This conversation was interrupted and is being resumed by a new agent process.
The filesystem and working directory may have changed since the last turn.
Previous tool results reflect the state at the time they were executed and may be stale.
Re-read any files you need before making changes.
```

This is lightweight, honest, and lets the model decide what to re-verify. The model already has the full conversation context and can reason about what might be stale.

### Step 4: Incomplete Turn Handling

If the journal ends mid-turn (crash during tool execution), the reconstructed messages may be in an inconsistent state:

| Last Entry Type | State | Action |
|----------------|-------|--------|
| `Message { role: User }` | Request was about to be sent | Resume from here — the user message is the pending turn |
| `Message { role: Assistant }` with `stop_reason: ToolUse` | Tools were about to execute | The assistant asked for tools but they never ran. Re-run the tools. |
| `Message { role: Assistant }` with `stop_reason: EndTurn` | Conversation was complete | Skip resume — just mark the task as done |
| `ToolExecution` (one or more) | Partial tool execution | Some tools ran, some didn't. **Drop the incomplete turn**: remove the last assistant message and any tool execution entries after it. The next API call will regenerate the assistant response. |
| `Init` only | Conversation never started | Start fresh with the initial user message |

The "drop incomplete turn" strategy is safe because:
- Tool executions are idempotent for read operations (file reads, searches)
- For write operations (file edits, bash commands), re-running is preferable to having a half-executed state
- The model will see the conversation up to the last clean turn and can decide what to do next

### Step 5: Context Budget Management (Compaction)

When the reconstructed messages exceed the model's context window, the resume loader must compact.

**Token estimation:**  
Rather than calling a tokenizer (which is model-specific and adds a dependency), use a simple heuristic:
- 1 token ≈ 4 characters for English text
- JSON structure overhead: +20% over raw text content
- Tool inputs/outputs: counted at the character-to-token ratio

This is deliberately conservative (overestimates tokens). The exact tokenizer can be added later as an optimization.

**Compaction algorithm:**

```
1. Estimate total tokens in messages + system prompt + tools
2. If total < model_context_window * 0.8:  → no compaction needed
3. Otherwise:
   a. Identify the "compaction window": messages from index 0 to N
      where N is chosen such that removing them brings total under budget
      (keep the most recent messages, compact the oldest)
   b. Summarize the compaction window:
      - Concatenate all text content and tool interactions
      - Ask the provider for a summary (using a small, separate request):
        "Summarize this conversation prefix. Focus on: decisions made,
         files modified, current state of the task, and any open questions."
      - Or, if provider call is too expensive/slow, use a mechanical summary:
        "Turns 1-N: [list of tools called with names and key inputs].
         Key findings: [text from assistant messages, truncated]."
   c. Write a Compaction entry to the journal
   d. Replace messages[0..N] with a single User message containing the summary
   e. Prepend a marker: "[Compacted: turns 1-N summarized. Details may be lossy.]"
```

**Budget thresholds:**

| Model Context Window | Compaction Trigger | Target After Compaction |
|---------------------|-------------------|----------------------|
| ≤ 8K tokens | 80% full | 50% full |
| 8K–32K tokens | 80% full | 60% full |
| 32K–128K tokens | 85% full | 65% full |
| > 128K tokens | 90% full | 70% full |

Larger context windows can tolerate higher utilization because the absolute headroom is larger.

**When compaction happens:**
- At resume time (before the first new API call)
- During a live conversation (before any `send()` call), if the pre-flight token estimate exceeds the trigger threshold

**Compaction is lossy and that's OK.** The journal preserves the full history on disk. Compaction only affects the messages array sent to the API. An auditor or debugger can always read the raw journal.

---

## 5. Tool Abstraction

### Current State (Already Unified)

The tool system is already provider-agnostic:

```rust
// tools/mod.rs — existing, unchanged
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;       // ← canonical type
    async fn execute(&self, input: &serde_json::Value) -> ToolOutput;
}
```

The `ToolDefinition` type (name, description, input_schema as JSON Schema) is the canonical format. The `OpenAiClient` translates this to `{ type: "function", function: { name, description, parameters } }` in its `translate_tools()` method. The `AnthropicClient` sends it directly.

**No changes needed to the tool abstraction.** The current design is correct:
- `ToolRegistry` manages tool definitions and dispatch
- `ToolDefinition` is the interchange format between agent loop and providers
- `Tool::execute()` returns `ToolOutput { content: String, is_error: bool }`
- Providers translate `ToolDefinition` → wire format in their `send()` implementation

### Tool Calls in the Journal

Tool calls are captured in three places in the journal:

1. **Assistant message**: Contains `ContentBlock::ToolUse { id, name, input }` — this is what the model requested.
2. **ToolExecution entry**: Contains the full input, output, timing, and error status — this is what actually happened.
3. **User message**: Contains `ContentBlock::ToolResult { tool_use_id, content, is_error }` — this is what the model saw as a result.

This triple-capture is intentional:
- (1) and (3) are needed for conversation replay (they're the messages array)
- (2) is needed for auditing, debugging, and evaluation (it has timing and the raw output before any truncation)

### Provider-to-Canonical Translation Points

Translation happens at the `Provider` boundary and **only** at the `Provider` boundary:

```
AgentLoop / ConversationManager
    ↕ canonical types (Message, ContentBlock, ToolDefinition, Usage)
Provider::send()
    ↕ wire format (provider-specific)
HTTP / API endpoint
```

The `ConversationManager` never sees wire format. The `Journal` never sees wire format. This is already true today and must remain true.

For a new provider (e.g., Google Gemini), the implementor would:
1. Implement `Provider` trait
2. Translate `MessagesRequest` (canonical) → Gemini wire format in `send()`
3. Translate Gemini response → `MessagesResponse` (canonical) in `send()`
4. Nothing else changes. The journal, resume, and tool system work automatically.

---

## 6. Migration Path

### Phase 1: Journal (Non-Breaking Addition)

**Files to create:**
- `src/executor/native/journal.rs` — `Journal` struct, `JournalEntry` types, read/write
- `src/executor/native/conversation.rs` — `ConversationManager` struct

**Files to modify:**
- `src/executor/native/mod.rs` — add `pub mod journal; pub mod conversation;`
- `src/executor/native/agent.rs` — replace `messages: Vec<Message>` with `ConversationManager`

**Strategy:**
- `ConversationManager` wraps the existing `Vec<Message>` pattern
- The `AgentLoop` constructor takes an optional journal path
- If a journal path is provided, `ConversationManager` writes entries
- If no journal path, behavior is identical to today (in-memory only)
- The existing `LogEvent` NDJSON writer (`agent.ndjson`) remains as-is — it serves a different purpose (observability summary) and can be removed later when the journal fully replaces it
- `StreamWriter` remains as-is — it serves the TUI/real-time display

**Backward compatibility:** 100%. The journal is opt-in. Existing code paths are unchanged.

**Task:** `impl-conversation-journal` (downstream consumer)

### Phase 2: Resume (Non-Breaking Addition)

**Files to create:**
- `src/executor/native/resume.rs` — `ResumeLoader` struct

**Files to modify:**
- `src/executor/native/agent.rs` — `AgentLoop::new()` accepts optional `ResumedConversation`
- `src/commands/native_exec.rs` — add `--resume` flag that looks for existing journal
- `src/commands/spawn/execution.rs` — pass journal path to native executor

**Strategy:**
- `ResumeLoader::load(journal_path) -> Result<ResumedConversation>` is a pure function
- `AgentLoop::new()` can accept a `ResumedConversation` to pre-populate the `ConversationManager`
- The coordinator can detect a failed task with a journal and dispatch a new agent with `--resume`
- First resume implementation can skip compaction (just fail if the journal is too large for the context window)

**Backward compatibility:** 100%. Resume is opt-in.

### Phase 3: Context Budget (Enhancement)

**Files to create:**
- `src/executor/native/context_budget.rs` — token estimation, compaction triggers

**Files to modify:**
- `src/executor/native/conversation.rs` — integrate `ContextBudget` into `send()` pre-flight check

**Strategy:**
- Start with the character-based heuristic (1 token ≈ 4 chars)
- Wire into `ConversationManager::send()` — before sending, check if compaction is needed
- First compaction strategy: mechanical summary (no LLM call), just enumerate tools called and truncate text
- LLM-based summarization can be added later

**Backward compatibility:** 100%. Budget management is internal to `ConversationManager`.

### Phase 4: Deprecate Legacy Logging (Cleanup)

Once the journal is proven stable:
- Remove `LogEvent` types and the `agent.ndjson` writer from `agent.rs`
- The journal supersedes all the information in `agent.ndjson` with higher fidelity
- `StreamWriter` (`stream.jsonl`) stays — it serves the TUI and has a different audience

**This phase is optional and can be deferred indefinitely.** The legacy logger and the journal can coexist.

### Summary: What Changes and What Doesn't

| Component | Changes? | Notes |
|-----------|----------|-------|
| `Provider` trait | **No** | Already correct |
| `AnthropicClient` | **No** | Already implements Provider |
| `OpenAiClient` | **No** | Already implements Provider with translation |
| `ToolRegistry` / `Tool` trait | **No** | Already provider-agnostic |
| `ContentBlock`, `Message`, `Usage` | **No** | Already the canonical types |
| `AgentLoop` | **Yes** (Phase 1) | Replace bare `Vec<Message>` with `ConversationManager` |
| `create_provider()` routing | **No** | Already correct |
| Config / endpoint system | **No** | Already correct |

The key insight: **the existing architecture is 80% right.** The `Provider` trait, canonical types, and translation layer are sound. The only gap is persistence and lifecycle management of the conversation — which is exactly what `ConversationManager` + `Journal` add.

---

## 7. Worked Example

### Scenario

A native executor agent runs task `fix-login-bug`. The model is `openai/gpt-4o` via OpenRouter. The agent reads a file, edits it, and runs a test.

### Journal File: `.wg/output/fix-login-bug/conversation.jsonl`

Each line below is a single JSON object. Formatted here for readability with `// comments` (not present in actual file).

```jsonl
{"seq":1,"timestamp":"2026-03-23T15:00:00.000Z","entry_type":"init","model":"openai/gpt-4o","provider":"openai","system_prompt":"You are a software engineer...","tools":[{"name":"read_file","description":"Read a file","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}},{"name":"edit_file","description":"Edit a file","input_schema":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}},{"name":"bash","description":"Run a shell command","input_schema":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}}],"task_id":"fix-login-bug"}
{"seq":2,"timestamp":"2026-03-23T15:00:00.100Z","entry_type":"message","role":"user","content":[{"type":"text","text":"Fix the login bug in src/auth.rs. The issue is that expired tokens are not rejected."}],"usage":null,"response_id":null,"stop_reason":null}
{"seq":3,"timestamp":"2026-03-23T15:00:02.500Z","entry_type":"message","role":"assistant","content":[{"type":"text","text":"I'll look at the auth module to understand the current token validation logic."},{"type":"tool_use","id":"call_001","name":"read_file","input":{"path":"src/auth.rs"}}],"usage":{"input_tokens":1250,"output_tokens":85,"cache_creation_input_tokens":null,"cache_read_input_tokens":null},"response_id":"chatcmpl-abc123","stop_reason":"tool_use"}
{"seq":4,"timestamp":"2026-03-23T15:00:02.550Z","entry_type":"tool_execution","tool_use_id":"call_001","name":"read_file","input":{"path":"src/auth.rs"},"output":"pub fn validate_token(token: &str) -> bool {\n    // TODO: check expiry\n    !token.is_empty()\n}\n","is_error":false,"duration_ms":45}
{"seq":5,"timestamp":"2026-03-23T15:00:02.600Z","entry_type":"message","role":"user","content":[{"type":"tool_result","tool_use_id":"call_001","content":"pub fn validate_token(token: &str) -> bool {\n    // TODO: check expiry\n    !token.is_empty()\n}\n","is_error":false}],"usage":null,"response_id":null,"stop_reason":null}
{"seq":6,"timestamp":"2026-03-23T15:00:05.200Z","entry_type":"message","role":"assistant","content":[{"type":"text","text":"I can see the issue — the token validation doesn't check expiry. Let me fix this."},{"type":"tool_use","id":"call_002","name":"edit_file","input":{"path":"src/auth.rs","content":"pub fn validate_token(token: &str) -> bool {\n    if token.is_empty() {\n        return false;\n    }\n    let claims = decode_token(token).ok();\n    claims.map_or(false, |c| c.exp > Utc::now().timestamp())\n}\n"}}],"usage":{"input_tokens":1450,"output_tokens":120,"cache_creation_input_tokens":null,"cache_read_input_tokens":null},"response_id":"chatcmpl-def456","stop_reason":"tool_use"}
{"seq":7,"timestamp":"2026-03-23T15:00:05.280Z","entry_type":"tool_execution","tool_use_id":"call_002","name":"edit_file","input":{"path":"src/auth.rs","content":"pub fn validate_token(token: &str) -> bool {\n    if token.is_empty() {\n        return false;\n    }\n    let claims = decode_token(token).ok();\n    claims.map_or(false, |c| c.exp > Utc::now().timestamp())\n}\n"},"output":"File written: src/auth.rs","is_error":false,"duration_ms":12}
{"seq":8,"timestamp":"2026-03-23T15:00:05.300Z","entry_type":"message","role":"user","content":[{"type":"tool_result","tool_use_id":"call_002","content":"File written: src/auth.rs","is_error":false}],"usage":null,"response_id":null,"stop_reason":null}
{"seq":9,"timestamp":"2026-03-23T15:00:07.800Z","entry_type":"message","role":"assistant","content":[{"type":"text","text":"Now let me run the tests to verify the fix."},{"type":"tool_use","id":"call_003","name":"bash","input":{"command":"cargo test test_auth"}}],"usage":{"input_tokens":1680,"output_tokens":45,"cache_creation_input_tokens":null,"cache_read_input_tokens":null},"response_id":"chatcmpl-ghi789","stop_reason":"tool_use"}
{"seq":10,"timestamp":"2026-03-23T15:00:12.100Z","entry_type":"tool_execution","tool_use_id":"call_003","name":"bash","input":{"command":"cargo test test_auth"},"output":"running 3 tests\ntest test_auth_valid_token ... ok\ntest test_auth_empty_token ... ok\ntest test_auth_expired_token ... ok\n\ntest result: ok. 3 passed; 0 failed","is_error":false,"duration_ms":4300}
{"seq":11,"timestamp":"2026-03-23T15:00:12.150Z","entry_type":"message","role":"user","content":[{"type":"tool_result","tool_use_id":"call_003","content":"running 3 tests\ntest test_auth_valid_token ... ok\ntest test_auth_empty_token ... ok\ntest test_auth_expired_token ... ok\n\ntest result: ok. 3 passed; 0 failed","is_error":false}],"usage":null,"response_id":null,"stop_reason":null}
{"seq":12,"timestamp":"2026-03-23T15:00:14.000Z","entry_type":"message","role":"assistant","content":[{"type":"text","text":"Fixed the login bug. The `validate_token` function now checks token expiry using the `exp` claim. All 3 auth tests pass."}],"usage":{"input_tokens":1850,"output_tokens":40,"cache_creation_input_tokens":null,"cache_read_input_tokens":null},"response_id":"chatcmpl-jkl012","stop_reason":"end_turn"}
{"seq":13,"timestamp":"2026-03-23T15:00:14.050Z","entry_type":"end","reason":"complete","total_usage":{"input_tokens":6230,"output_tokens":290,"cache_creation_input_tokens":null,"cache_read_input_tokens":null},"turns":4}
```

### Resume Scenario

Suppose the agent crashed after seq 10 (tool execution for the test command completed, but the tool result message was never sent). The journal on disk has entries seq 1–10.

**Resume reconstruction:**

1. Load entries 1–10
2. From `Init` (seq 1): restore system_prompt, tools, model, provider
3. From `Message` entries: rebuild messages array:
   - messages[0] = User (seq 2): initial prompt
   - messages[1] = Assistant (seq 3): read_file request
   - messages[2] = User (seq 5): read_file result
   - messages[3] = Assistant (seq 6): edit_file request
   - messages[4] = User (seq 8): edit_file result
   - messages[5] = Assistant (seq 9): bash request
4. Last entry is `ToolExecution` (seq 10) — the tool ran but the result was never sent
5. **Action**: Construct the tool result from the ToolExecution entry and add it as the next user message:
   - messages[6] = User: `ToolResult { tool_use_id: "call_003", content: "running 3 tests...", is_error: false }`
6. Inject stale-state notice as an additional text block in messages[6]
7. Continue the agent loop from this point — the next API call will include all 7 messages

### Compaction Scenario

Suppose the journal has 200 turns and the reconstructed messages are 90K tokens against a 128K context window (70% full, which is under the 85% trigger for 32–128K windows). No compaction needed.

But if it were 115K tokens (90% — above the 85% trigger):
1. Target: 65% of 128K = ~83K tokens
2. Need to shed ~32K tokens
3. Walk from oldest messages forward until removing enough
4. Say messages[0..40] account for 35K tokens
5. Generate mechanical summary: "Turns 1–20: Read files src/a.rs, src/b.rs, src/c.rs. Edited src/a.rs (added error handling). Ran cargo test (3 failures). Read error logs..."
6. Write `Compaction { compacted_through_seq: 80, summary: "...", original_message_count: 40, original_token_count: 35000 }` to journal
7. Replace messages[0..40] with a single User message: "[Compacted: turns 1–20 summarized...] {summary}"
8. New total: ~80K tokens (62.5%) — under target

---

## Appendix A: ConversationConfig

```rust
pub struct ConversationConfig {
    /// Path to the journal file.
    pub journal_path: PathBuf,
    /// Model name (for context window lookup).
    pub model: String,
    /// Provider name (for journal metadata).
    pub provider: String,
    /// System prompt.
    pub system_prompt: String,
    /// Tool definitions.
    pub tools: Vec<ToolDefinition>,
    /// Task ID (optional, for journal metadata).
    pub task_id: Option<String>,
    /// Maximum context window size in tokens (from model registry).
    /// If None, no compaction is performed.
    pub max_context_tokens: Option<u32>,
}
```

## Appendix B: File Layout

```
src/executor/native/
├── mod.rs              ← add: pub mod journal; pub mod conversation; pub mod resume; pub mod context_budget;
├── agent.rs            ← modify: use ConversationManager instead of Vec<Message>
├── client.rs           ← UNCHANGED (canonical types)
├── openai_client.rs    ← UNCHANGED (OpenAI translation)
├── provider.rs         ← UNCHANGED (Provider trait + routing)
├── bundle.rs           ← UNCHANGED
├── conversation.rs     ← NEW: ConversationManager
├── journal.rs          ← NEW: Journal, JournalEntry, JournalEntryKind
├── resume.rs           ← NEW: ResumeLoader
├── context_budget.rs   ← NEW: ContextBudget, token estimation
└── tools/
    ├── mod.rs          ← UNCHANGED
    ├── bash.rs         ← UNCHANGED
    ├── file.rs         ← UNCHANGED
    └── wg.rs           ← UNCHANGED
```

## Appendix C: Decision Log

| Decision | Rationale | Alternatives Considered |
|----------|-----------|------------------------|
| Retain existing canonical types as internal representation | Already provider-agnostic, well-tested, used by both providers | New "neutral" message type → unnecessary translation layer |
| JSONL for journal format | Append-friendly, human-readable, matches existing NDJSON patterns | SQLite (overkill, not human-readable), protobuf (not human-readable) |
| Journal at task-id path, not agent-id | Survives agent restarts, enables multi-attempt resume | Agent-id path (current pattern for agent.ndjson) → lost on retry |
| Stale-state notice instead of re-validation | Cheap, lets model decide what to re-check, avoids dangerous re-execution | Re-execute all tool calls (expensive, dangerous), validate file hashes (partial, complex) |
| Character-based token estimation | No external dependency, good enough for compaction triggers | Tiktoken (adds dependency, model-specific), API-based counting (latency) |
| ToolExecution as separate entry type | Captures timing and full I/O; Message entries only have what the API sees | Inline in Message (loses timing), separate log file (splits the record) |
| Mechanical compaction before LLM-based | Simpler, deterministic, no API cost | LLM-first (expensive, may fail, recursive dependency on the conversation layer itself) |
| ConversationManager owns messages | Clear ownership, single point of persistence, agent loop stays simple | Agent loop owns messages + calls journal separately (scattered responsibility) |
