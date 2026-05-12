# Agent Message Queue Design

## Problem Statement

Currently there is no way to communicate with a running agent after it has been spawned.
If the task description is updated or new context arrives, the agent continues with stale
information. We need:

1. A message queue per task that accumulates messages
2. Running agents that can receive new messages mid-execution
3. Pending tasks with queued messages that agents read when they claim the task
4. A clear producer and consumer API

## Architecture Overview

The system has three layers:

```
┌─────────────────────────────────────────────────────┐
│  Producer API (wg msg send)                         │
│  - CLI command for humans and agents                │
│  - IPC request for coordinator/federation           │
└──────────────────────┬──────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────┐
│  Message Store (.wg/messages/{task-id}.jsonl) │
│  - Append-only JSONL files                          │
│  - One file per task                                │
│  - Atomic append via O_APPEND                       │
└──────────────────────┬──────────────────────────────┘
                       │
┌──────────────────────▼──────────────────────────────┐
│  Consumer Layer (executor adapters)                  │
│  - Claude adapter: wrapper script polls & injects   │
│  - Amplifier adapter: API injection                 │
│  - Shell adapter: env var / file path               │
│  - Agent self-poll: wg msg poll <task-id>           │
└─────────────────────────────────────────────────────┘
```

## 1. Message Format and Storage

### Message Format

```rust
/// A single message in the queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// Unique message ID (monotonic counter per task, e.g. "1", "2", "3")
    pub id: u64,
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Sender identifier: "user", "coordinator", agent-id, or task-id
    pub sender: String,
    /// Message body (free-form text, may contain markdown)
    pub body: String,
    /// Priority: "normal" (default) or "urgent"
    #[serde(default = "default_priority")]
    pub priority: String,
}
```

### Storage Location

```
.wg/messages/
├── task-alpha.jsonl      # messages for task-alpha
├── task-beta.jsonl       # messages for task-beta
└── .cursors/             # per-agent read cursors
    ├── agent-1.task-alpha   # "3" (last read message ID)
    └── agent-2.task-beta    # "1"
```

### Why JSONL (not extending `wg log`)

The existing `wg log` mechanism stores entries inside the graph YAML file. This has
problems for messaging:

1. **Contention**: The graph file is a single shared resource. Appending a log entry
   requires loading the entire graph, modifying it, and saving it back. Two concurrent
   writers (agent + coordinator) can clobber each other.

2. **Semantics**: Log entries are observational records ("I did X"). Messages are
   imperative directives ("Please do Y"). Mixing them would create ambiguity about
   which entries an agent should act on.

3. **Read cursors**: Messages need per-agent read tracking. Log entries don't.

4. **Performance**: JSONL append is O(1) via `O_APPEND` (atomic on POSIX for
   writes ≤ `PIPE_BUF`). Graph load/save is O(n) where n = graph size.

JSONL files are independent of the graph, support concurrent append without locking,
and can be read incrementally.

### Storage Operations

```
Append:  open(path, O_WRONLY | O_APPEND | O_CREAT) → write(json + "\n")
Read:    open(path, O_RDONLY) → read lines → parse JSON
Cursor:  read/write .cursors/{agent-id}.{task-id} (contains last-read msg ID)
```

## 2. Producer API

### CLI: `wg msg send`

```bash
# Send a message to a task (any task, any status)
wg msg send <task-id> "message text"

# Send with explicit sender
wg msg send <task-id> "message text" --from coordinator

# Send urgent message
wg msg send <task-id> "message text" --priority urgent

# Pipe message body from stdin
echo "long context..." | wg msg send <task-id> --stdin
```

Implementation: append a `Message` line to `.wg/messages/{task-id}.jsonl`.

The `id` field is assigned by reading the current file to find the highest existing ID
and incrementing by 1. If the file doesn't exist, start at 1. This is safe because
the coordinator is the only process creating new messages at scale, and CLI sends are
human-rate. For true concurrent safety, a simple flock() around the read-max-id +
append sequence suffices.

### IPC: `SendMessage` request

Add to the existing `IpcRequest` enum:

```rust
IpcRequest::SendMessage {
    task_id: String,
    body: String,
    sender: Option<String>,
    priority: Option<String>,
}
```

This lets the coordinator, federation peers, and tooling send messages without
shelling out to `wg msg send`.

### Programmatic (Rust)

```rust
pub fn send_message(
    workgraph_dir: &Path,
    task_id: &str,
    body: &str,
    sender: &str,
    priority: &str,
) -> Result<u64>  // returns message ID
```

## 3. Consumer API

### CLI: `wg msg list` / `wg msg read` / `wg msg poll`

```bash
# List all messages for a task
wg msg list <task-id>
wg msg list <task-id> --json

# Read unread messages (advances cursor for current agent)
wg msg read <task-id>
wg msg read <task-id> --agent agent-5

# Poll for new messages (returns 0 if new messages, 1 if none)
# Designed for wrapper script polling loops
wg msg poll <task-id> --agent agent-5
wg msg poll <task-id> --agent agent-5 --json
```

### How `wg msg read` works

1. Read cursor from `.wg/messages/.cursors/{agent-id}.{task-id}`
   (defaults to 0 if no cursor exists)
2. Read messages from `.wg/messages/{task-id}.jsonl`
3. Filter to messages with `id > cursor`
4. Print them
5. Update cursor to max message ID seen

### How `wg msg poll` works

Same as `read` but:
- Exit code 0 = new messages exist (prints them)
- Exit code 1 = no new messages
- Designed for `while true; do wg msg poll ...; sleep 5; done` loops

## 4. Executor Adapter Interface

The key challenge is: how do different executors inject messages into a running agent?
Each executor type has different capabilities.

### Adapter Trait

```rust
/// Defines how an executor delivers messages to a running agent.
pub trait MessageAdapter {
    /// Deliver a message to a running agent.
    /// Returns Ok(true) if delivered, Ok(false) if agent can't receive,
    /// Err if delivery failed.
    fn deliver(&self, agent: &AgentEntry, message: &Message) -> Result<bool>;

    /// Whether this adapter supports real-time injection (vs polling).
    fn supports_realtime(&self) -> bool;
}
```

### Claude Executor Adapter

**Current state**: Agents are launched with `claude --print` piped from a prompt
file, with `stdin(Stdio::null())`. The process runs to completion.

**Option A — Wrapper script polling (recommended for v1)**:

The wrapper `run.sh` already runs the agent command and handles exit. We modify it
to run a background polling loop that checks for new messages:

```bash
#!/bin/bash
TASK_ID='my-task'
AGENT_ID='agent-5'
OUTPUT_FILE='.wg/agents/agent-5/output.log'

# Background: poll for messages and write to a signal file
poll_messages() {
    while true; do
        if wg msg poll "$TASK_ID" --agent "$AGENT_ID" --json > /tmp/wg-msg-$AGENT_ID.new 2>/dev/null; then
            # New messages arrived — but we can't inject them into claude --print
            # Log them so the agent sees them next time it reads its output
            echo "[wg] New messages received:" >> "$OUTPUT_FILE"
            cat /tmp/wg-msg-$AGENT_ID.new >> "$OUTPUT_FILE"
        fi
        sleep 10
    done
}

# The agent command (unchanged)
cat prompt.txt | claude --print --verbose ... >> "$OUTPUT_FILE" 2>&1
EXIT_CODE=$?

# ... existing wrapper logic ...
```

**Limitation**: `claude --print` in text input mode reads stdin once and processes
a single turn. It cannot receive additional user messages mid-session. The polling
loop above can only log messages to the output file, which the agent won't see
unless it happens to read its own output file.

**Option B — Stream-JSON input mode (recommended for v2)**:

Claude CLI supports `--input-format stream-json` which keeps the process alive and
accepts new user messages via stdin as NDJSON:

```json
{"type":"user","message":{"role":"user","content":"New context: the API changed"}}
```

To use this, the spawn flow changes:
1. Launch claude with `--input-format stream-json --output-format stream-json`
2. Keep the stdin pipe open (change `Stdio::null()` to `Stdio::piped()`)
3. Store the stdin write handle in the agent registry or a sidecar file
4. When a message arrives, write the NDJSON user message to the pipe

**Challenge**: The spawned process is detached via `setsid()` and the parent
(coordinator) doesn't hold handles. Solutions:
- **Named pipe (FIFO)**: Create `.wg/agents/{agent-id}/inbox.fifo`, launch
  claude reading from this FIFO, write messages to it from any process.
- **Stdin relay process**: A small relay daemon holds the pipe and accepts messages
  via a unix socket or FIFO.

**Recommendation**: Start with Option A (polling + file-based notification) for
v1. Plan for Option B (stream-json + named pipe) for v2 once the message
infrastructure is proven.

### Amplifier Executor Adapter

Amplifier runs in `--mode single` with text output. It does not currently support
mid-execution message injection.

**Option A — Same polling approach as Claude**: wrapper script polls, but can't
inject into the running amplifier process.

**Option B — Amplifier API**: If/when amplifier exposes an API for injecting
context into a running session, the adapter would call that API.

**For v1**: Same file-based approach as Claude. Messages accumulate in the queue;
the agent can call `wg msg read` if instructed to poll periodically.

### Shell Executor Adapter

Shell tasks run arbitrary commands. They can:
1. Call `wg msg read` themselves
2. Check `$WG_MSG_FILE` environment variable pointing to the message queue file
3. Use inotifywait on the message file for real-time notification

### Agent Self-Poll Pattern

The most universal approach: instruct agents to periodically check for messages.
Add to the prompt template:

```
## Messages
Check for new messages periodically during long-running tasks:
```bash
wg msg poll {{task_id}} --agent $WG_AGENT_ID
```
```

This works with ALL executor types because it's agent-driven.

## 5. Integration with Coordinator Service

### On task claim (spawn time)

When the coordinator spawns an agent, it should:

1. Read any queued messages for the task from `.wg/messages/{task-id}.jsonl`
2. Include them in the prompt context:

```
## Queued Messages
The following messages were sent to this task before you started:

[2026-02-28T22:00:00Z] user: Please focus on the error handling edge cases
[2026-02-28T22:15:00Z] coordinator: Related task 'auth-refactor' just completed, see artifacts
```

3. Set the agent's read cursor to the last message ID

### On message arrival (for running agents)

When `wg msg send` is called for a task with a running agent:

1. Append the message to the JSONL file
2. Optionally notify the agent via the executor adapter (v2)
3. If adapter can't deliver, the message stays queued for the agent's next poll

### Coordinator-generated messages

The coordinator can send messages automatically:

- When a dependency completes: "Dependency 'task-X' completed. New artifacts available: [list]"
- When a sibling task produces relevant output: "Sibling task 'task-Y' logged: [summary]"
- When the user updates the task description: "Task description updated. New requirements: [diff]"
- When the graph structure changes: "New downstream consumer added: 'task-Z'"

### IPC integration

Add `SendMessage` to the IPC protocol so that:
- Federation peers can send cross-repo messages
- The coordinator can send messages without CLI overhead
- Tooling can inject messages programmatically

## 6. Edge Cases

### Agent dies before reading messages

Messages persist in the JSONL file. When the task is retried and a new agent spawns,
it starts with a fresh cursor (0) and receives all messages, including those the
previous agent never read.

The read cursor is per-agent, so a new agent-id gets a fresh cursor automatically.

### Message sent to completed task

Allowed. The JSONL file accepts appends regardless of task status. This supports:
- Post-completion feedback ("your output had a bug, FYI")
- Retrospective analysis
- Messages that arrive due to race conditions

The `wg msg list` command works on any task regardless of status.

### Message sent to non-existent task

Return an error: "Task '{id}' not found". We validate against the graph before
appending.

### Concurrent writers

JSONL with `O_APPEND` is safe for concurrent writes on POSIX systems when each
write is ≤ `PIPE_BUF` (4096 bytes on Linux). For messages that might exceed this:
- Use a flock() advisory lock around the write
- Or write to a temp file and rename (like the registry does)

In practice, messages are short text and won't exceed `PIPE_BUF`.

### Message ordering

Messages are ordered by their `id` field (monotonically incrementing per task).
Timestamps provide wall-clock ordering but `id` is authoritative for sequencing.

### Garbage collection

Add a `wg msg gc` command or extend `wg gc` to clean up message files for:
- Tasks that are Done/Failed/Abandoned and older than N days
- Tasks that have been deleted from the graph

### Large message queues

For tasks that accumulate many messages (e.g., long-running loop tasks):
- `wg msg read` only returns unread messages (cursor-based)
- Add `--limit N` flag to cap output
- Consider rotation (archive old messages) for tasks with 1000+ messages

## 7. Prompt Integration

### Prompt section for queued messages

When building the agent prompt in `executor.rs::build_prompt()`, add a new section
after "Context from Dependencies":

```rust
// Messages section (task+ scope)
if scope >= ContextScope::Task && !ctx.queued_messages.is_empty() {
    parts.push(ctx.queued_messages.clone());
}
```

The `ScopeContext` struct gains a `queued_messages: String` field, populated during
spawn by reading the message queue.

### Agent instructions

Add to the `REQUIRED_WORKFLOW_SECTION`:

```
6. **Check for messages** during long tasks:
   ```bash
   wg msg poll {{task_id}}
   ```
   Messages may contain updated requirements, context from other agents,
   or instructions from the user.
```

## 8. Implementation Plan

### Phase 1: Core messaging (v1)

Files to create/modify:
- `src/messages.rs` — `Message` struct, storage operations (append, read, cursor)
- `src/commands/msg.rs` — CLI commands (send, list, read, poll)
- `src/commands/mod.rs` — register msg subcommand
- `src/cli.rs` — wire up CLI args
- `src/commands/spawn/execution.rs` — inject queued messages into prompt at spawn time
- `src/service/executor.rs` — add `queued_messages` to `ScopeContext`
- `src/commands/service/ipc.rs` — add `SendMessage` IPC request

### Phase 2: Agent awareness (v1.1)

- Update prompt templates to instruct agents to poll for messages
- Coordinator auto-sends messages when dependencies complete
- `wg msg gc` for cleanup

### Phase 3: Real-time injection (v2)

- Named pipe (FIFO) per agent for stdin relay
- `--input-format stream-json` support in Claude executor
- `MessageAdapter` trait and per-executor implementations
- Wrapper script spawns relay daemon alongside agent

### Phase 4: Advanced features

- Message threading (reply-to-id)
- Binary attachments (file references)
- Message expiration (TTL)
- Rate limiting
- Encryption for cross-repo federation messages

## 9. Comparison of Approaches

| Approach | Latency | Complexity | Works with --print | Works with stream-json | Cross-executor |
|---|---|---|---|---|---|
| File-based JSONL + polling | ~10s | Low | Yes (via self-poll) | Yes | Yes |
| Named pipe / FIFO | ~instant | Medium | No | Yes | No |
| Unix socket per agent | ~instant | High | No | Yes | No |
| Extend wg log | N/A | Low | Yes | Yes | Yes |
| Shared file + inotify | ~instant | Medium | Yes (via wrapper) | Yes | Partially |

**Recommendation**: File-based JSONL for storage, agent self-polling for consumption
in v1. Named pipe + stream-json for real-time injection in v2.

## 10. File-Based Queue: Detailed Specification

### Directory structure

```
.wg/
├── messages/
│   ├── {task-id}.jsonl          # message queue per task
│   └── .cursors/
│       └── {agent-id}.{task-id} # plain text file containing last-read msg ID
├── agents/
│   └── {agent-id}/
│       ├── run.sh
│       ├── prompt.txt
│       ├── output.log
│       └── metadata.json
```

### JSONL file format

Each line is a complete JSON object:

```jsonl
{"id":1,"timestamp":"2026-02-28T22:00:00Z","sender":"user","body":"Focus on error handling","priority":"normal"}
{"id":2,"timestamp":"2026-02-28T22:15:00Z","sender":"coordinator","body":"Dependency auth-refactor completed","priority":"normal"}
{"id":3,"timestamp":"2026-02-28T22:30:00Z","sender":"agent-3","body":"Found related issue in parser.rs","priority":"urgent"}
```

### Cursor file format

Plain text containing a single u64 — the last-read message ID:

```
3
```

If the file doesn't exist, the cursor is 0 (all messages are unread).

### Concurrency model

- **Writers** (wg msg send, coordinator): `O_APPEND` for atomic single-line writes
- **Readers** (wg msg read/poll): read-only, no locking needed
- **Cursor updates**: write-to-temp + rename (atomic, like registry.json)
- **ID assignment**: flock() the JSONL file, read max ID, assign next, append, unlock

This model supports unlimited concurrent readers and serialized writers (via flock
for ID assignment).
