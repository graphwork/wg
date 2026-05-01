# Design: Coordinator Chat History Browsing & Context Loading

**Task:** design-coordinator-chat
**Date:** 2026-03-27
**Status:** Design complete

---

## 1. Current State Analysis

### 1.1 Chat Persistence — Dual-Layer Architecture

Chat data is stored in two independent layers:

| Layer | Location | Format | Purpose |
|-------|----------|--------|---------|
| **IPC layer** | `.wg/chat/{coordinator_id}/inbox.jsonl` | JSONL (ChatMessage) | User → coordinator messages |
| **IPC layer** | `.wg/chat/{coordinator_id}/outbox.jsonl` | JSONL (ChatMessage) | Coordinator → user responses |
| **IPC layer** | `.wg/chat/{coordinator_id}/chat.log` | Plaintext | Grep-friendly full log |
| **TUI layer** | `.wg/chat-history.json` | JSON array | Display persistence across TUI restarts |

Each coordinator gets its own subdirectory under `.wg/chat/`. The IPC layer is the **source of truth** — inbox/outbox JSONL files with `flock`-based concurrency, cursor files for read tracking, and coordinator cursors for consumption tracking.

### 1.2 Key Data Structures

**IPC-level `ChatMessage`** (`src/chat.rs:37`):
```rust
struct ChatMessage {
    id: u64,              // Monotonic within file
    timestamp: String,    // ISO 8601
    role: String,         // "user" | "coordinator"
    content: String,      // Summary text
    request_id: String,   // Correlates request↔response
    attachments: Vec<Attachment>,
    full_response: Option<String>,  // Full text including tool calls
    user: Option<String>,
}
```

**TUI-level `ChatMessage`** (`src/tui/viz_viewer/state.rs:1055`):
```rust
struct ChatMessage {
    role: ChatRole,       // User | Coordinator | System
    text: String,
    full_text: Option<String>,
    attachments: Vec<String>,
    edited: bool,
    inbox_id: Option<u64>,
    user: Option<String>,
}
```

### 1.3 Current TUI Behavior

- On startup, `load_chat_history()` tries `chat-history.json` first, falls back to `inbox.jsonl`/`outbox.jsonl` for coordinator 0.
- During runtime, `poll_chat_messages()` checks outbox mtime and reads new messages via cursor.
- Per-coordinator chat states are stored in `coordinator_chats: HashMap<u32, ChatState>` and swapped on coordinator tab switch.
- **Bug**: `chat-history.json` is a single global file, NOT per-coordinator. Only coordinator 0's history survives TUI restarts. Other coordinators load empty on restart.
- **Bug**: `save_chat_history()` writes `chrono::Utc::now()` as the timestamp for all persisted messages, losing the original timestamp.

### 1.4 Compaction System

The compactor produces `.wg/compactor/context.md` via LLM summarization of graph state. It's injected into coordinator context on each user message via `build_coordinator_context()`. This is a **project-level** summary, not a **conversation-level** summary.

### 1.5 Crash Recovery

On coordinator agent restart, `build_crash_recovery_summary()` loads the last 10 messages from `inbox.jsonl`/`outbox.jsonl` and injects them along with current graph state. This is the closest existing analogue to "context loading from history."

---

## 2. Browsing UX Recommendation

### 2.1 Recommendation: Scrollback + Session Markers (Timeline Model)

**Chosen approach:** Extend the existing chat view with unbounded scrollback and session boundary markers, rather than introducing a separate history browser modal.

**Rationale:**
- Users already scroll within the chat panel — extending this is zero new interaction to learn
- A separate modal/tab adds navigation overhead and splits attention
- Session markers naturally organize conversation flow without requiring explicit "conversation" boundaries
- Aligns with how terminal chat applications work (Discord, Slack, iMessage)

### 2.2 UX Specification

```
┌─ Chat: Coordinator: erik ─────────────────────────────┐
│                                                        │
│  ── Session: Mar 26, 14:02 ──────────────────────────  │
│  14:02 you: Create a task to fix the auth bug     ✓✓   │
│  14:02 coordinator: Created fix-auth-bug with...  ✓    │
│  14:03 you: Also add a test task for it           ✓✓   │
│  14:04 coordinator: Added test-auth-fix --after.. ✓    │
│                                                        │
│  ── Session: Mar 27, 09:15 ──────────────────────────  │
│  09:15 you: What's the status of fix-auth-bug?    ✓✓   │
│  09:17 coordinator: fix-auth-bug is done. test... ✓    │
│  09:22 you: Show me the agents                    ⟳    │
│  09:22 ░░░░░░ coordinator is processing... (4s)        │
│  you: █                                                │
│                                                        │
│  ─────────────────────────────────────────── ▓ 87/102  │
└────────────────────────────────────────────────────────┘
```

**Message State Indicators** (right-aligned badges):
| Symbol | State | Meaning |
|--------|-------|---------|
| `·` | **Sent** | Message written to inbox, not yet seen by coordinator |
| `✓` | **Seen** | Coordinator's inbox cursor has advanced past this message |
| `✓✓` | **Responded** | Coordinator has written a response (correlated by request_id) |
| `⟳` | **Processing** | Coordinator is actively generating a response (streaming in progress) |

**How state is derived:**
- **Sent → Seen**: Compare message ID against `.coordinator-cursor` file. The coordinator advances this cursor after reading each inbox message.
- **Seen → Processing**: When `.streaming` file exists and is non-empty for the active coordinator.
- **Processing → Responded**: When `outbox.jsonl` contains a message with matching `request_id`.
- Coordinator messages show `✓` (delivered to TUI) once the TUI's outbox cursor passes them.

**Timestamps:**
- Every message shows `HH:MM` timestamp (derived from the ISO 8601 timestamp in the JSONL record).
- Full date shown in session markers, relative time on hover (if mouse support enabled).
- Original message timestamps preserved, not overwritten (fixes current bug where `save_chat_history` uses `Utc::now()`).

**Other Elements:**
- **Session markers**: Horizontal dividers with date/time, inserted when there's a gap > N minutes (configurable, default 30min) between consecutive messages.
- **Scrollback**: Up-arrow / PgUp / mouse scroll loads older messages from the IPC-layer JSONL files, paginated to avoid loading entire history at once.
- **Scroll position indicator**: `line/total` in the scrollbar gutter to show position in history.
- **Search** (`/` in Normal mode): Fuzzy search across all messages, highlighting matches and jumping to them.
- **Processing indicator**: Animated spinner + elapsed time shown while `awaiting_response` is true, replacing the current bare "awaiting..." state.

### 2.3 Rejected Alternatives

| Alternative | Why Rejected |
|-------------|-------------|
| Separate "History" tab | Splits chat context, adds navigation overhead; users would need to switch back and forth |
| Modal dialog with conversation list | Requires defining "conversation" boundaries (non-trivial), heavy UX for browsing |
| Timeline view as separate panel | Screen real estate already constrained by graph + inspector + chat split |

---

## 3. Context Loading Mechanism

### 3.1 Overview

Three distinct use cases, with three mechanisms:

| Use Case | Mechanism | UX |
|----------|-----------|-----|
| **Resume** a previous coordinator session | Auto-recovery from stored history | Automatic on coordinator start |
| **Inject** specific past messages as context | Manual selection → context injection | User-driven via keybinding |
| **Cross-pollinate** from another coordinator's chat | Copy-to-context action | User-driven via coordinator tab |

### 3.2 Mechanism A: Auto-Resume (Crash Recovery Enhancement)

**Current state:** Crash recovery already injects last 10 messages. This should become the default "session resume" path.

**Enhancement:**
1. On coordinator agent startup (not just crash restart), inject a configurable number of recent messages as conversation history.
2. Instead of sending as a single user message (current approach), use the Claude CLI `--resume` session or inject as `system`-role context to avoid polluting the conversation.
3. Include compacted context.md (already done) plus recent chat summary (new).

**Config surface:**
```toml
[coordinator]
# Number of recent messages to include in context on start/restart
context_history_count = 20
# Whether to include full_response (tool calls) or just summaries
context_include_full = false
```

**CLI surface:**
```bash
# Resume a coordinator with explicit history depth
wg service start --history-depth 50

# Start fresh (no history injection)
wg service start --no-history
```

### 3.3 Mechanism B: Manual Context Injection

Allow users to select messages from history and inject them as context for the current coordinator conversation.

**UX flow:**
1. User enters history selection mode: `Ctrl+H` in the chat panel
2. Chat scrollback appears with checkboxes on each message
3. User navigates with arrow keys, toggles selection with `Space`
4. `Enter` confirms selection → selected messages are injected as a system context block
5. `Esc` cancels

**API/CLI surface:**
```bash
# Inject specific messages from another coordinator's history
wg chat --inject-from <coordinator_id> --last 20

# Inject from a saved context file
wg chat --inject-context <path>

# Export a coordinator's chat for sharing/archival
wg chat --export > conversation.md
wg chat --export --json > conversation.jsonl
```

**Implementation:**
The injected messages are formatted as a system context block and sent via the existing `UserChat` IPC:
```
[Context from previous session (Mar 26 14:02 – Mar 26 14:47):]
User: Create a task to fix the auth bug
Coordinator: Created fix-auth-bug with deps on...
[End context]

Based on this context, here is my new question: ...
```

### 3.4 Mechanism C: Cross-Coordinator Context

When viewing a coordinator's chat, the user can "send to" another coordinator:

**UX:** Right-click or keybinding on a message → "Send to Coordinator N" → Message appears as injected context in target coordinator's chat.

This is a Phase 3 feature (see implementation plan below).

---

## 4. Context Selection Strategies

### 4.1 Strategy Comparison

| Strategy | Pros | Cons | Token Cost | Accuracy |
|----------|------|------|------------|----------|
| **Recency** (last N messages) | Simple, predictable, zero compute | Misses relevant older messages, includes irrelevant recent ones | Proportional to N | Medium |
| **Task-Relevance** (messages referencing current tasks) | Highly targeted, low noise | Requires task-ID extraction from messages; misses general discussion | Low (filtered) | High for task work |
| **Compacted Summary** (LLM-generated) | Maximal signal density, fixed token budget | Adds LLM call latency, may lose details | Fixed (~2K tokens) | High (but lossy) |
| **Hybrid: Compacted + Recency** | Best of both — persistent knowledge + recent detail | Two sources to merge | ~2K + last-N | Highest |

### 4.2 Recommendation: Hybrid (Compacted + Recency)

**Phase 1 (MVP):** Recency-only with configurable depth. Simple, zero additional infrastructure.

**Phase 2:** Add conversation compaction that produces a per-coordinator summary alongside the existing project-level compaction. This leverages the same LLM pipeline (`run_lightweight_llm_call`) but operates on chat history rather than graph state.

**Phase 3:** Task-relevance filtering as an enhancement — when the coordinator has active tasks, prefer messages that reference those task IDs.

### 4.3 Conversation Compaction (Phase 2 Detail)

Parallel to the existing `.compact-0` cycle for project context, add a per-coordinator conversation compactor:

**Storage:** `.wg/chat/{coordinator_id}/context-summary.md`

**Trigger:** When `inbox.jsonl` + `outbox.jsonl` exceed a threshold (default: 100 messages or 50K tokens), the compactor runs and produces a rolling summary.

**Prompt template:**
```
You are summarizing a conversation between a user and a workgraph coordinator.
Preserve:
- Decisions made and their rationale
- User preferences and recurring instructions
- Task IDs and their context
- Any unresolved questions or pending actions

Conversation:
{messages}

Output: A structured summary under 2000 tokens.
```

**Integration with crash recovery:** Replace the raw "last 10 messages" with `context-summary.md` + last 5 messages. This gives the coordinator persistent knowledge of the entire conversation history within a bounded token budget.

---

## 5. Data Flow Diagram

```
                          ┌──────────────────────────┐
                          │   User Input (TUI/CLI)   │
                          └────────┬─────────────────┘
                                   │
                                   ▼
                     ┌─────────────────────────┐
                     │  IPC: UserChat request   │
                     │  → inbox.jsonl append    │
                     └────────┬────────────────┘
                              │
                              ▼
                ┌──────────────────────────────┐
                │  Coordinator Agent Process    │
                │  ┌─────────────────────────┐ │
                │  │ Context Injection:       │ │
                │  │  • context.md (project)  │ │
                │  │  • context-summary.md    │ │  ◄── Phase 2
                │  │    (conversation)        │ │
                │  │  • graph summary         │ │
                │  │  • recent events         │ │
                │  └─────────────────────────┘ │
                │  ┌─────────────────────────┐ │
                │  │ LLM Response             │ │
                │  └────────┬────────────────┘ │
                └───────────┼──────────────────┘
                            │
                            ▼
                ┌──────────────────────────────┐
                │  outbox.jsonl append          │
                │  + chat.log plaintext         │
                │  + .streaming partial         │
                └────────┬─────────────────────┘
                         │
               ┌─────────┴───────────┐
               ▼                     ▼
    ┌──────────────────┐  ┌─────────────────────┐
    │  TUI: poll_chat  │  │  CLI: wait_for_     │
    │  _messages()     │  │  response()         │
    │  ↓               │  └─────────────────────┘
    │  Display in chat │
    │  panel + save to │
    │  chat-history    │
    │  .json           │
    └──────────────────┘

    ┌──────────────────────────────────────────────┐
    │  History Browsing (TUI):                     │
    │                                              │
    │  chat-history.json  ←── TUI display layer    │
    │       ↑ (recent)        (fast, in-memory)    │
    │                                              │
    │  inbox.jsonl +      ←── IPC layer            │
    │  outbox.jsonl           (source of truth,    │
    │       ↑ (all history)    paginated loading)  │
    │                                              │
    │  context-summary.md ←── Compacted summary    │
    │       ↑                  (Phase 2)           │
    │  Conversation compactor                      │
    └──────────────────────────────────────────────┘
```

---

## 6. Storage & Retention Strategy

### 6.1 Current Rotation

`rotate_history_for()` already keeps the last N messages per JSONL file (default 200 per file, so 200 inbox + 200 outbox = 400 messages retained). This is called on coordinator agent restart.

### 6.2 Recommended Strategy

| Tier | Retention | Storage | Purpose |
|------|-----------|---------|---------|
| **Hot** | Last 1000 messages | `inbox.jsonl` + `outbox.jsonl` | Active history, browsable in TUI |
| **Warm** | Rotated archives | `inbox.1.jsonl`, `inbox.2.jsonl` | Searchable, not loaded by default |
| **Cold** | Compacted summary | `context-summary.md` | Always-available conversation knowledge |

### 6.3 Archive Rotation (Phase 2)

Instead of dropping messages when rotating, rename the old file:
```
inbox.jsonl → inbox.1.jsonl (previous)
inbox.1.jsonl → inbox.2.jsonl (older)
inbox.2.jsonl → (deleted after N generations, default 3)
```

This preserves full history for search while keeping the hot path fast.

### 6.4 Config Surface

```toml
[coordinator]
# Messages to keep in active JSONL files
chat_rotation_keep = 1000

# Number of archive generations to retain
chat_archive_generations = 3

# Trigger conversation compaction when message count exceeds this
chat_compact_threshold = 100
```

---

## 7. Implementation Plan

### Phase 1: MVP — Fix Persistence + Scrollback (1–2 tasks)

**Goal:** Chat history reliably survives TUI restarts for ALL coordinators; scrollback works smoothly.

| Task | Description | Files | Deps |
|------|-------------|-------|------|
| **fix-chat-history-per-coordinator** | Make `chat-history.json` per-coordinator (`chat/{id}/chat-history.json`). Fix timestamp bug (use original message timestamp, not `Utc::now()`). Load correct file on coordinator switch. | `src/tui/viz_viewer/state.rs` | None |
| **tui-chat-scrollback** | Paginated loading from JSONL when user scrolls past the in-memory buffer. Add session boundary markers based on timestamp gaps. Add scroll position indicator. | `src/tui/viz_viewer/state.rs`, `render.rs`, `event.rs` | fix-chat-history-per-coordinator |

| **tui-message-lifecycle** | Add per-message timestamps (HH:MM) and message state indicators (sent/seen/processing/responded). Derive state from coordinator-cursor and request_id correlation. Show processing spinner with elapsed time. | `src/tui/viz_viewer/render.rs`, `state.rs` | fix-chat-history-per-coordinator |

**Validation:**
- TUI restart preserves chat for all coordinators (not just 0)
- Scrolling up past loaded messages triggers JSONL load
- Session markers appear at 30+ minute gaps
- Scroll position shows current/total
- Each message shows HH:MM timestamp
- User messages show sent/seen/responded state indicators
- Processing spinner appears during coordinator response generation with elapsed time

### Phase 2: Context Loading + Conversation Compaction (2–3 tasks)

**Goal:** Coordinators automatically get relevant conversation context; users can manually inject context.

| Task | Description | Files | Deps |
|------|-------------|-------|------|
| **auto-resume-context** | Enhance coordinator agent startup to inject configurable number of recent messages as context (not just crash recovery). Add `--history-depth` and `--no-history` flags. | `src/commands/service/coordinator_agent.rs`, `src/config.rs`, `src/cli.rs` | Phase 1 |
| **conversation-compactor** | Per-coordinator conversation compaction: summarize chat history into `context-summary.md`. Run when message count exceeds threshold. Integrate with crash recovery (replace raw messages with summary + last N). | `src/service/compactor.rs` (or new `src/service/chat_compactor.rs`), `src/commands/service/coordinator_agent.rs` | auto-resume-context |
| **manual-context-inject** | `Ctrl+H` history selection mode in TUI. `wg chat --inject-from` CLI. `wg chat --export` for archival. | `src/tui/viz_viewer/event.rs`, `src/tui/viz_viewer/render.rs`, `src/commands/chat.rs`, `src/cli.rs` | Phase 1 |

**Validation:**
- New coordinator session automatically includes recent conversation context
- Conversation compaction produces a summary under 2K tokens
- Crash recovery uses compacted summary + last 5 messages
- `Ctrl+H` opens selection mode, selected messages appear as context
- `wg chat --export` produces valid markdown

### Phase 3: Advanced Features (2–3 tasks)

**Goal:** Cross-coordinator context sharing, search, archive rotation.

| Task | Description | Files | Deps |
|------|-------------|-------|------|
| **chat-search** | `/` key in chat panel triggers fuzzy search across all messages (including archived JSONL). Highlight matches, jump between results. | `src/tui/viz_viewer/event.rs`, `render.rs`, `state.rs` | Phase 1 |
| **archive-rotation** | Numbered archive rotation instead of drop-on-rotate. Config for generations and thresholds. | `src/chat.rs`, `src/config.rs` | Phase 1 |
| **cross-coordinator-context** | Send messages from one coordinator's chat to another as context. UX: keybinding on message → "Send to C{n}". | `src/tui/viz_viewer/event.rs`, `src/commands/chat.rs` | manual-context-inject |

**Validation:**
- Search finds messages across full history including archives
- Rotation preserves N generations of archive files
- Cross-coordinator injection delivers message as context block

---

## 8. Dependency Graph

```
Phase 1:
  fix-chat-history-per-coordinator
        │
        ▼
  tui-chat-scrollback ──────────────────────────┐
        │                                       │
Phase 2:│                                       │
        ├──▶ auto-resume-context                │
        │         │                             │
        │         ▼                             │
        │    conversation-compactor             │
        │                                       │
        └──▶ manual-context-inject              │
                   │                            │
Phase 3:           │                            │
                   ▼                            │
              cross-coordinator-context         │
                                                │
        ┌───────────────────────────────────────┘
        │
        ├──▶ chat-search
        │
        └──▶ archive-rotation
```

---

## 9. Open Questions / Future Considerations

1. **Multi-user chat**: When multiple users share a coordinator, should each user see only their own messages or all messages? Currently `user` field is stored but not filtered on. (Defer to multi-user design.)

2. **Conversation compaction quality**: Should we expose a "regenerate summary" command for when the LLM produces a poor summary?

3. **Token budget**: How much of the coordinator's context window should conversation history consume? Should it compete with graph context or have a reserved allocation?

4. **Bookmarks/pins**: Users might want to pin important messages so they always appear in compacted summaries. This could be a Phase 4 feature.

5. **Task-relevance scoring**: In Phase 3, could use embedding-based similarity between current tasks and past messages for smarter context selection. Requires an embedding API call — worth the latency?
