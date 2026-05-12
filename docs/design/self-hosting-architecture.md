# Self-Hosting wg: Architecture Design

## Status: Design (March 2026)

## Vision

wg becomes its own primary interface. Users interact with the system through the system itself — no separate Claude Code window on top. Two complementary interaction modes: a persistent coordinator agent that lives inside the graph and accepts user messages, and a TUI that serves as a full control surface for viewing, creating, editing, and communicating.

## Current State

### What exists

| Component | Location | Status | Relevance |
|-----------|----------|--------|-----------|
| Service daemon | `src/commands/service/mod.rs` | Working | UnixSocket IPC, settling delay, poll loop |
| Coordinator tick | `src/commands/service/coordinator.rs` | Working | Auto-assign, auto-eval, agent spawning |
| IPC protocol | `src/commands/service/ipc.rs` | Working | Spawn, Kill, Status, GraphChanged, SendMessage, AddTask, Pause/Resume, Reconfigure |
| Message queue | `src/messages.rs`, `src/commands/msg.rs` | Working | JSONL storage, cursors, send/list/read/poll CLI |
| Executor system | `src/service/executor.rs` | Working | ExecutorRegistry, prompt assembly, 4 tiers (shell/bare/light/full) |
| TUI viz viewer | `src/tui/viz_viewer/` | Working | Search, task selection, HUD, edge tracing, live refresh |
| Agency system | `src/agency/` | Working | Roles, tradeoffs, agents, auto-assign, auto-evaluate, lineage |
| Agent prompt | `src/service/executor.rs` | Working | Scope-based assembly, stigmergy instructions |

### What's missing for self-hosting

1. **Persistent coordinator agent** — The current coordinator is pure Rust code that ticks periodically. There's no LLM session that can interpret user intent, create tasks from natural language, or provide conversational interaction.

2. **Instant wake-up** — The daemon uses 100ms non-blocking accept + settling delay for IPC-triggered ticks. For user conversation, we need sub-second response from message send to coordinator awareness.

3. **TUI as control surface** — The TUI is currently a read-only viewer. It can't create tasks, edit dependencies, send messages, or host a chat interface.

4. **Executor independence** — Current executors depend on Claude Code CLI or Amplifier (Python). For true self-hosting, we need a Rust-native executor that calls LLM APIs directly.

5. **User↔coordinator chat** — No mechanism for the user to send a message and get a conversational response from the coordinator agent.

## Architecture

### System Overview

```
┌─────────────────────────────────────────────────────────────┐
│                         TUI                                  │
│  ┌──────────┐  ┌──────────────┐  ┌───────────────────────┐  │
│  │ Graph Viz │  │ Task Panel   │  │ Chat Panel            │  │
│  │ (existing)│  │ Create/Edit  │  │ User ↔ Coordinator    │  │
│  │           │  │ Dependencies │  │ Message log           │  │
│  └──────────┘  └──────────────┘  └───────────────────────┘  │
└─────────────────────┬───────────────────────────────────────┘
                      │ wg msg send / wg add / IPC
┌─────────────────────▼───────────────────────────────────────┐
│                   Service Daemon                             │
│  ┌─────────────────┐  ┌────────────────────────────────┐    │
│  │ IPC Listener     │  │ Coordinator Agent              │    │
│  │ (UnixSocket)     │  │ (persistent LLM session)       │    │
│  │                  │  │                                │    │
│  │ GraphChanged ────┼──▶ Wake-up (instant)              │    │
│  │ SendMessage ─────┼──▶ Process message → respond      │    │
│  │ UserChat ────────┼──▶ Interpret → create tasks       │    │
│  └─────────────────┘  │ Monitor agents → report status  │    │
│                        └────────────────────────────────┘    │
│  ┌─────────────────────────────────────────────────────┐    │
│  │ Task Executor Pool                                   │    │
│  │ (unchanged: spawn agents for ready tasks)            │    │
│  └─────────────────────────────────────────────────────┘    │
└─────────────────────────────────────────────────────────────┘
```

### 1. Coordinator Agent Lifecycle

The coordinator agent is a persistent LLM session that lives inside the service daemon. Unlike regular task agents (which are spawned, do work, and exit), the coordinator agent:

- **Starts when the service starts** and persists across the service lifetime
- **Maintains conversation context** — it knows graph state, recent completions, agent activities
- **Wakes instantly on messages** — not polling, but event-driven via the IPC channel
- **Has tools** — can call `wg add`, `wg edit`, `wg show`, `wg list`, `wg msg send`, etc.
- **Responds to the user** — sends responses back through the message queue

#### Communication Model

```
User (via TUI or CLI)                    Coordinator Agent
         │                                      │
         │── wg chat "help me plan auth" ──────▶│
         │                                      │ (LLM processes)
         │                                      │ (calls wg add, wg list, etc.)
         │◀── response via message queue ───────│
         │                                      │
         │── wg chat "also add rate limiting" ─▶│
         │                                      │ (maintains context from above)
         │◀── response ────────────────────────│
```

#### Implementation Options

**Option A: Stream-JSON Claude CLI session (recommended for v1)**

Launch a Claude CLI process with `--input-format stream-json --output-format stream-json`. Keep stdin piped open. Send user messages as NDJSON `{"type":"user","message":{"role":"user","content":"..."}}`. Parse responses from stdout.

Pros: Uses existing Claude Code tooling. Agent has full tool access. Context is managed by Claude.
Cons: Depends on Claude CLI. Process management complexity.

**Option B: Rust-native LLM client (v2)**

A Rust HTTP client that calls the Anthropic API directly, with tool use support. The coordinator agent's tools are implemented as Rust functions that call `wg` operations in-process (no CLI shelling out).

Pros: No external dependencies. In-process tool calls are faster. Full control over context management.
Cons: Must implement tool use loop, context management, streaming. Significant development effort.

**Option C: Hybrid — start with CLI, migrate to native**

Use Option A for rapid iteration. Build the Rust native executor (Option B) as a parallel effort. Switch the coordinator to native once it's stable.

**Recommendation: Option C.** Ship a working coordinator agent quickly with Claude CLI, then invest in the native executor for long-term independence.

#### Coordinator Agent Context Management

The coordinator needs awareness of:
- **Graph snapshot** — current task statuses, dependencies, agents (refreshed on each wake)
- **Recent events** — which tasks completed/failed since last interaction
- **User conversation history** — maintained by the LLM session
- **Running agents** — who's working on what (from agent registry)

On each wake-up, inject a context update:
```
## System Context Update
- Graph: 45 tasks (30 done, 5 in-progress, 8 open, 2 blocked)
- Since last interaction: task-alpha completed, task-beta failed (reason: test failure)
- Active agents: agent-1 on task-gamma (running 5m), agent-2 on task-delta (running 12m)
```

#### Wake-up Mechanism

Current daemon loop: 100ms non-blocking socket accept → process IPC → check settling deadline → tick.

For coordinator wake-up, add a **dedicated channel** between the IPC handler and the coordinator agent:

1. IPC handler receives `UserChat` or `SendMessage` to a coordinator-watched task
2. Handler writes message to a coordinator inbox (in-memory channel or file-based FIFO)
3. Coordinator agent's stdin relay picks up the message and injects it as a new user turn
4. Response streams back, gets captured, sent to response queue
5. TUI/CLI polls response queue or gets notified

The settling delay mechanism already exists for GraphChanged events. For user chat, we want **zero settling delay** — immediate injection.

### 2. TUI Extensions

#### Current TUI Architecture

The TUI (`src/tui/viz_viewer/`) is a ratatui app with:
- `state.rs` — `VizApp` struct with all state (scroll, search, selection, HUD)
- `event.rs` — keyboard/mouse event handling
- `render.rs` — frame rendering

It currently renders the graph viz output as colored text and provides search, navigation, task selection, and an info HUD panel.

#### New TUI Panels

The TUI needs to evolve from a single-panel viz viewer to a multi-panel control surface:

```
┌─────────────────────────────────────────────────────────────┐
│ Status: 45 tasks (30✓ 5⟳ 8○ 2✗) │ 3 agents │ Service: ● │
├──────────────────────────┬──────────────────────────────────┤
│                          │ ▸ Chat with Coordinator          │
│    Graph Visualization   │──────────────────────────────────│
│    (existing viz panel)  │ user: help me plan the auth      │
│                          │   system for our app             │
│                          │                                  │
│                          │ coordinator: I'll create a task  │
│                          │   plan for authentication:       │
│                          │   1. Research auth patterns...   │
│                          │   2. Implement JWT middleware... │
│                          │                                  │
│                          │ > _                              │
├──────────────────────────┼──────────────────────────────────┤
│ Task Detail (HUD)        │ Quick Actions                    │
│ ID: auth-research        │ [a] Add task  [e] Edit           │
│ Status: in-progress      │ [d] Done      [f] Fail           │
│ Agent: agent-12 (5m)     │ [m] Message   [r] Retry          │
│ Deps: design-spec ✓      │ [l] Link dep  [/] Search         │
└──────────────────────────┴──────────────────────────────────┘
```

#### TUI Feature Breakdown

**a) Task creation panel** — Press `a` to open a task creation form:
- Title (required)
- Description (multiline editor, optional)
- Dependencies (`--after`, with fuzzy task search)
- Tags, skills
- Exec mode selector (shell/bare/light/full)

**b) Task editing** — Press `e` on selected task to modify:
- Description
- Dependencies (add/remove)
- Status changes
- Metadata

**c) Chat panel** — Right-side panel for coordinator conversation:
- Scrollable message history
- Text input at bottom
- Messages sent via `wg chat` (new command)
- Responses displayed as they stream in

**d) Quick actions** — Context-sensitive actions on selected task:
- Done/Fail/Retry with confirmation
- Send message to task's agent
- View agent output log

**e) Agent monitor** — Overlay or panel showing:
- Active agents and their tasks
- Uptime, token usage
- Output tailing

### 3. Executor Independence

#### Current Executor Landscape

| Executor | Backend | Dependencies | Tier Support |
|----------|---------|--------------|--------------|
| `claude` | Claude Code CLI | Node.js, npm, claude CLI | full, light, bare |
| `amplifier` | Amplifier Python | Python, pip, amplifier | full (bundles) |
| `shell` | bash | None | shell only |

#### Rust-Native Executor Design

A new `native` executor that calls LLM APIs directly from Rust:

```rust
pub struct NativeExecutor {
    client: reqwest::Client,
    api_key: String,
    model: String,
    tools: Vec<ToolDefinition>,
}

impl NativeExecutor {
    /// Run a single-turn or multi-turn agent conversation.
    /// Tools are executed in-process via the wg library.
    pub async fn run_agent(
        &self,
        prompt: &str,
        tools: &[ToolDefinition],
        max_turns: usize,
    ) -> Result<AgentResult> {
        // 1. Send initial message to API
        // 2. Process tool_use blocks
        // 3. Execute tools (wg add, wg show, cargo build, etc.)
        // 4. Return tool results
        // 5. Loop until agent says "done" or max_turns
    }
}
```

#### Tool System for Native Executor

The native executor needs a tool registry mapping tool names to implementations:

- **wg tools**: `wg_add`, `wg_show`, `wg_list`, `wg_edit`, `wg_done`, `wg_fail`, `wg_log`, `wg_artifact`, `wg_msg_send`, `wg_msg_read` — call wg library functions directly (no CLI subprocess)
- **File tools**: `read_file`, `write_file`, `edit_file`, `glob`, `grep` — direct filesystem access
- **Shell tool**: `bash` — subprocess execution with timeout
- **Web tool**: `web_fetch` — HTTP client

This maps closely to Claude Code's tool set, so prompts are portable.

#### Bundle/Capability System

Bundles define what tools and context an agent gets. This replaces the current ad-hoc `--allowedTools` flags:

```toml
# .wg/bundles/research.toml
name = "research"
description = "Read-only research agent"
tools = ["read_file", "glob", "grep", "web_fetch", "wg_show", "wg_list", "wg_log", "wg_done"]
context_scope = "graph"
system_prompt_suffix = "You are a research agent. Report findings."

# .wg/bundles/implementer.toml
name = "implementer"
description = "Full implementation agent"
tools = ["*"]
context_scope = "full"
```

Bundles are referenced in executor config and map to exec_mode tiers:
- `shell` tier → no bundle (direct command execution)
- `bare` tier → `bare` bundle (wg tools only)
- `light` tier → `research` bundle (read-only + wg)
- `full` tier → `implementer` bundle (all tools)

### 4. Message Passing and Instant Wake-Up

#### Current State

Message queue is fully implemented:
- `wg msg send <task> "text"` — send message
- `wg msg list/read/poll` — consume messages
- IPC `SendMessage` — programmatic sending
- JSONL storage with per-agent cursors

#### What's Missing

1. **`wg chat` command** — Send a message to the coordinator agent and wait for a response
2. **Coordinator inbox** — A special message channel for coordinator communication
3. **Response delivery** — Coordinator's responses need to reach the user (TUI or CLI)
4. **Instant wake-up** — Current GraphChanged settling delay (default 2000ms) is too slow for chat

#### Design: Coordinator Chat Protocol

```
# User sends chat message
wg chat "create a task for implementing JWT auth"

# Under the hood:
# 1. wg chat sends IPC UserChat request to daemon
# 2. Daemon injects message into coordinator agent's stdin (stream-json)
# 3. Coordinator agent processes, calls tools (wg add, etc.)
# 4. Coordinator agent produces a text response
# 5. Response is written to .wg/chat/response-{id}.txt
# 6. wg chat reads response and displays it

# For TUI:
# Same flow but the chat panel handles send/receive asynchronously
```

#### IPC Extension

```rust
IpcRequest::UserChat {
    message: String,
    /// Unique request ID for correlating responses
    request_id: String,
}

IpcRequest::ChatResponse {
    request_id: String,
}
```

#### File-Based Response Channel

```
.wg/chat/
├── inbox.jsonl           # User messages to coordinator
├── outbox.jsonl          # Coordinator responses to user
└── .cursor               # Last-read response ID
```

Both TUI and CLI `wg chat` read from `outbox.jsonl` and advance the cursor.

### 5. Stigmergy Patterns

#### Core Principle

Every agent operates on the graph as a shared medium. The graph is the coordination substrate — agents discover work by reading the graph, and create work by writing to it. No centralized task assignment is needed beyond initial dispatch; agents self-organize through the graph.

#### Current Stigmergic Behaviors

Already present in the skill/quickstart:
- "The graph is alive" section in agent prompts
- Task decomposition instructions (fan-out, pipeline, integrator)
- `wg add --after` for creating follow-up work

#### Missing Stigmergic Patterns

1. **Discovery trails** — When an agent finds something interesting (a bug, a pattern, a dependency), it should leave a breadcrumb in the graph. Not just a log entry (ephemeral), but a task or artifact (persistent, actionable).

2. **Pheromone-like signals** — Task tags and metadata that attract agents. A task tagged `needs-review` attracts reviewer agents. A task tagged `blocked:missing-doc` creates an affordance for a documentation agent.

3. **Self-spawning work** — The agent skill should more strongly emphasize: "You are expected to create tasks. If you see work that needs doing, add it. The coordinator will dispatch it."

4. **Cross-agent awareness** — Agents should be able to see what other agents recently completed and use those artifacts. The `wg context` command already provides dependency context, but sibling awareness is limited.

#### Implementation

- Update `SKILL.md` and quickstart with stronger stigmergy language
- Add `wg discover` command that shows recently completed tasks with artifacts (a "what's new" for agents)
- Add tag-based affinity to the assigner (tasks with certain tags prefer certain agent types)
- Coordinator agent should model stigmergic principles in its own behavior

### 6. Implementation Phases

#### Phase 1: Foundation (Message Passing + Wake-Up)
- Implement `wg chat` command (send to coordinator, receive response)
- Add `UserChat` IPC request type
- Implement coordinator inbox/outbox file channels
- Reduce settling delay for chat messages (or bypass it)
- Add response correlation (request-id → response matching)
- Integration tests for chat round-trip

#### Phase 2: Coordinator Agent
- Design coordinator agent prompt (system + context injection pattern)
- Implement coordinator agent spawning in daemon startup
- Stream-JSON stdin injection for user messages
- Context refresh on wake-up (graph summary, recent events, active agents)
- Response capture and routing to outbox
- Coordinator agent tool restrictions (inspect + create, no implement)
- Handle coordinator agent crashes (restart with context recovery)

#### Phase 3: TUI Control Surface
- Multi-panel layout (viz + side panel) with panel switching
- Task creation form (title, description, deps, tags)
- Chat panel (send messages, display responses)
- Quick actions on selected task (done, fail, retry, message)
- Task editing (description, deps)
- Agent output viewer
- Async rendering (don't block on coordinator responses)

#### Phase 4: Executor Independence
- Design Rust-native LLM client (Anthropic API)
- Implement tool-use loop (message → tool_call → execute → result → message)
- Implement core tool set (bash, read, write, edit, glob, grep)
- Implement wg tools as in-process library calls
- Bundle/capability system (TOML config, tool filtering)
- Register native executor alongside claude/shell
- Integration test: native executor runs a simple task end-to-end

#### Phase 5: Stigmergy
- Update SKILL.md with stigmergy patterns
- Add `wg discover` command (recent completions + artifacts)
- Tag-based affinity in assigner
- Agent-to-agent message patterns
- Self-spawning work reinforcement in prompts

#### Phase 6: Integration and Validation
- End-to-end test: user creates project via TUI chat → coordinator creates tasks → agents execute → work completes
- Dogfooding: use self-hosted wg to manage its own development
- Performance benchmarking (chat latency, coordinator overhead)
- Worktree audit (verify agent isolation, merge workflow)
- Multi-cycle stress test (loop tasks, failure restart, convergence)

## Dependency Map

```
Phase 1 (Foundation)
  ├── chat-command
  ├── chat-ipc-protocol
  ├── coordinator-inbox-outbox
  └── instant-wake-up
        │
Phase 2 (Coordinator Agent)  ◀── depends on Phase 1
  ├── coordinator-prompt-design
  ├── coordinator-spawn-in-daemon
  ├── stream-json-injection
  ├── context-refresh
  └── crash-recovery
        │
Phase 3 (TUI) ◀── depends on Phase 2 (for chat panel)
  ├── multi-panel-layout      (can start with Phase 1)
  ├── task-creation-form      (can start with Phase 1)
  ├── chat-panel              (needs Phase 2)
  ├── quick-actions           (independent)
  └── agent-output-viewer     (independent)
        │
Phase 4 (Executor Independence)  ◀── partially parallel with Phase 2-3
  ├── rust-llm-client
  ├── tool-use-loop
  ├── core-tool-set
  ├── wg-tools-in-process
  └── bundle-system
        │
Phase 5 (Stigmergy)  ◀── can start anytime
  ├── skill-update
  ├── discover-command
  └── tag-affinity
        │
Phase 6 (Validation)  ◀── depends on Phase 1-5
  ├── e2e-test
  ├── dogfooding
  ├── perf-benchmark
  └── stress-test
```

## Risk Assessment

| Risk | Impact | Mitigation |
|------|--------|------------|
| Claude CLI stream-json instability | Coordinator agent breaks | Watchdog restart + context recovery. Fall back to single-turn polling. |
| Coordinator context overflow | Degraded responses | Aggressive context pruning. Summary injection instead of full graph. |
| TUI complexity explosion | Hard to maintain | Incremental panels. Each panel is an independent module. |
| Rust native executor scope | Delays Phase 4 | Phase 4 is parallel and non-blocking. Ship with Claude CLI first. |
| Chat latency > 5s | Poor UX | Stream responses. Show "thinking..." indicator. |

## Open Questions

1. **Should the coordinator agent be a special task in the graph?** Treating it as a hidden task gives it an ID, logs, artifacts — fitting the stigmergic model. But it never "completes," which is unusual.

2. **How to handle coordinator context limits?** Over a long session, the coordinator accumulates conversation history. Options: periodic summarization, context window rotation, or restart with summary injection.

3. **Should TUI embed a terminal emulator for chat?** Or use a simpler message-and-response panel? Terminal emulator is more flexible but much more complex.

4. **How does the native executor interact with git worktrees?** Agents using worktrees need `git worktree add/remove` orchestration. The native executor needs this capability.

5. **Should bundles be TOML files or part of the agency system?** Bundles could be associated with roles (a "Researcher" role always gets the research bundle). This creates a clean mapping.
