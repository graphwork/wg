# Messaging System Research Report

## 1. Message Queue Implementation

### Storage Model

Messages are stored as **JSONL files** in `.wg/messages/{task-id}.jsonl`, one file per task. Read cursors live in `.wg/messages/.cursors/{agent-id}.{task-id}`.

**Source:** `src/messages.rs:1-4`

```rust
pub struct Message {
    pub id: u64,           // Monotonic counter per task
    pub timestamp: String, // ISO 8601 / RFC 3339
    pub sender: String,    // "user", "coordinator", agent-id, or task-id
    pub body: String,      // Free-form text, may contain markdown
    pub priority: String,  // "normal" (default) or "urgent"
}
```

### Key Storage Operations

| Operation | Implementation | Notes |
|-----------|---------------|-------|
| `send_message()` | Open file with `O_APPEND`, `flock()` for exclusive lock, read max ID, append JSON line | Thread-safe via flock |
| `list_messages()` | Read all JSONL lines, parse, sort by ID | Returns `Vec<Message>` |
| `read_cursor()` | Read plain-text file containing `u64` | Returns 0 if no cursor exists |
| `write_cursor()` | Write-to-temp + atomic rename | Safe against crashes |
| `read_unread()` | Filter messages with `id > cursor`, advance cursor to max seen | Destructive read |
| `poll_messages()` | Same filter as `read_unread` but does NOT advance cursor | Non-destructive peek |
| `format_queued_messages()` | Format all messages for prompt injection | Returns empty string if no messages |

### Design Rationale (vs extending `wg log`)

The design doc (`docs/design/agent-message-queue.md:74-93`) explains why JSONL was chosen over extending `wg log`:

1. **Contention**: Graph file is a shared resource; concurrent writers can clobber each other
2. **Semantics**: Logs are observational ("I did X"), messages are imperative ("Please do Y")
3. **Read cursors**: Messages need per-agent read tracking
4. **Performance**: JSONL append is O(1), graph load/save is O(n)

---

## 2. Agent-to-Agent Messaging

### Addressing Model: Task-Based

Messaging is **task-based, not agent-based**. You send a message to a task ID, and whichever agent is working on that task can read it.

```bash
wg msg send <task-id> "message text"              # Default sender: "user"
wg msg send <task-id> "text" --from agent-xyz     # Explicit sender
wg msg send <task-id> "text" --priority urgent    # Urgent priority
```

### Can Agent A Send a Message to Agent B?

**Yes, indirectly.** Agent A sends a message to Agent B's task:

```bash
wg msg send <agent-B-task-id> "Hey B, check this out" --from $WG_AGENT_ID
```

Agent B can then read it with:

```bash
wg msg read <task-id> --agent $WG_AGENT_ID
```

There is **no direct agent-to-agent addressing** — all messages are routed through task queues. This means:
- You need to know the target agent's task ID
- If the target task is reassigned to a different agent, the new agent sees the message (fresh cursor at 0)
- An agent working on multiple tasks would need to poll each task's queue separately (though currently agents work on one task at a time)

### Message Delivery to Running Agents

Three executor-specific adapters exist (`src/messages.rs:281-435`), but **none support real-time injection in v1**:

| Executor | Adapter | Supports Realtime | Delivery Mechanism |
|----------|---------|-------------------|--------------------|
| `claude` | `ClaudeMessageAdapter` | No | Writes `pending_messages.txt` notification file |
| `amplifier` | `AmplifierMessageAdapter` | No | Same notification file mechanism |
| `shell` | `ShellMessageAdapter` | No | Same notification file mechanism |

The `deliver_message()` function (`src/messages.rs:444-467`) is the main entry point:
1. Stores message in persistent JSONL queue
2. Attempts real-time delivery via the adapter (always returns `false` in v1)
3. Writes to `pending_messages.txt` in the agent's output directory

### Wrapper Script Polling

The wrapper `run.sh` (`src/commands/spawn/execution.rs:572-660`) includes a **background polling loop** that:
1. Every 10 seconds, checks if `pending_messages.txt` has content
2. Atomically moves the file to a temp location and appends its content to the agent's output log
3. Also calls `wg msg poll` and `wg msg read` to check the message queue directly
4. Appends any new messages to the output log with `[wg] === New messages received ===` markers

**Limitation:** This polling loop writes messages to the agent's output log file, but `claude --print` reads stdin once. The agent only sees these messages if it happens to read its own output file. In practice, this means **messages sent to running agents are logged but not reliably consumed by the LLM**.

---

## 3. Message Checking in Agents

### Prompt Injection at Spawn

When an agent is spawned, queued messages are included in the initial prompt:

**In `src/commands/spawn/context.rs:131-134`:**
```rust
// Task+ scope: queued messages
if scope >= ContextScope::Task {
    ctx.queued_messages = workgraph::messages::format_queued_messages(workgraph_dir, &task.id);
}
```

**In `src/service/executor.rs:259-261`:**
```rust
if scope >= ContextScope::Task && !ctx.queued_messages.is_empty() {
    parts.push(ctx.queued_messages.clone());
}
```

Pre-existing messages appear under a `## Queued Messages` header. This only works at scope >= Task (not Clean scope).

### Prompt Section Instructing Agents to Check

The `MESSAGE_POLLING_SECTION` constant (`src/service/executor.rs:166-174`) is injected into all task+ scope prompts:

```
## Messages

Check for new messages periodically during long-running tasks:
```bash
wg msg read {{task_id}} --agent $WG_AGENT_ID
```
Messages may contain updated requirements, context from other agents,
or instructions from the user. Check at natural breakpoints in your work.
```

This tells agents to check, but:
- It's a **soft instruction** — agents may or may not follow it
- There's **no enforced check** before `wg done`
- The quickstart (`src/commands/quickstart.rs`) does **NOT** mention messaging commands
- The SKILL.md does **NOT** mention `wg msg` commands at all

### Cursor Advancement at Spawn

When an agent spawns, the cursor should be advanced past pre-existing messages (since they're already in the prompt). The integration test (`tests/integration_messaging.rs:482-508`) verifies this pattern, but the actual cursor advancement in `execution.rs` relies on the agent's first `wg msg read` call rather than being done automatically at spawn time.

---

## 4. Testing

### Unit Tests (`src/messages.rs:470-858`)

16 unit tests covering:
- Send and list messages (basic CRUD)
- Empty queue handling
- Cursor read/write roundtrip
- `read_unread` advancing cursor
- `poll` NOT advancing cursor
- Separate cursors per agent
- Separate queues per task
- `format_queued_messages` (empty and populated)
- Message ordering
- Valid RFC 3339 timestamps
- Adapter factory (`adapter_for_executor`) for claude/amplifier/shell/unknown
- Claude adapter notification file writing
- Amplifier adapter notification file writing
- Notification accumulation (multiple messages)
- `deliver_message` stores and notifies
- Notification directory auto-creation

### Unit Tests (`src/commands/msg.rs:186-289`)

5 unit tests covering:
- `run_send` basic
- `run_send` to nonexistent task
- `run_send` with empty body
- `run_list` (text and JSON)
- `run_read` and `run_poll` interaction

### Integration Tests (`tests/integration_messaging.rs`, 1283 lines)

**7 test sections, ~30 tests total:**

1. **Message Storage** (5 tests): send/persist, ordering by ID, read/unread tracking, separate cursors, poll vs read, timestamps
2. **CLI Commands** (7 tests): send/list, JSON output, read advances cursor, poll exit codes, poll JSON, empty list, send-to-nonexistent
3. **Pending Task Pickup** (3 tests): queued messages in formatted context, cursor advancement at spawn, empty queue formatting
4. **Running Agent Delivery** (5 tests): Claude/Amplifier/Shell adapter notification files, adapter factory, notification accumulation
5. **Edge Cases** (8 tests): nonexistent task, completed task (allowed), agent dies before reading, agent dies after partial read, rapid succession, empty body rejected, empty queue list, nonexistent task for list/read/poll
6. **Coordinator Integration** (2 tests): deliver stores and notifies, multiple deliveries across tasks
7. **Smoke Tests** (3 tests): full e2e lifecycle, CLI-only flow, multi-task messaging

Plus 2 prompt-building tests verifying queued messages section inclusion/exclusion.

### Test Coverage Assessment

**Well-tested:**
- Core JSONL storage (send, list, cursor, unread, poll)
- CLI command I/O and error handling
- Executor adapter notification files
- Edge cases (nonexistent tasks, completed tasks, agent death/retry)
- End-to-end message lifecycle

**Not tested:**
- Concurrent write safety (flock under contention) — no multi-process test
- The wrapper script's message polling loop (it's a bash script, tested only by inspection)
- IPC `SendMessage` handler integration test (only tested via code review)
- Real `claude --print` agent receiving messages mid-execution
- Message garbage collection (`wg msg gc` doesn't exist yet)
- Very large message queues (performance)
- TUI `SendMessage` action (scaffolded but not wired — `src/tui/viz_viewer/event.rs:196`)

---

## 5. Executor Integration

### Claude Executor

- Agent runs `claude --print` which reads stdin once (single turn)
- Messages queued before spawn → included in prompt via `format_queued_messages()`
- Messages during execution → written to `pending_messages.txt` → wrapper polling loop moves them to output.log
- Agent can self-poll with `wg msg read`
- **No real-time injection possible** in v1

### Amplifier Executor

- Runs in `--mode single` with text output
- Same pattern as Claude: pre-spawn messages in prompt, mid-execution via notification file
- Agent can self-poll with `wg msg read`
- **No real-time injection possible** in v1

### Shell Executor

- Runs arbitrary commands
- Can call `wg msg read` directly
- `$WG_MSG_FILE` environment variable mentioned in design doc but **not confirmed in code** (not set in `execution.rs`)
- Wrapper script does the same polling loop

### IPC SendMessage

Implemented in `src/commands/service/ipc.rs:97-104, 738+`:
- Validates task exists in graph
- Calls `workgraph::messages::send_message()`
- Returns message ID and task ID
- Used by coordinator and federation peers for programmatic messaging

### Coordinator Auto-Messages

The design doc mentions the coordinator auto-sending messages when:
- Dependencies complete
- Sibling tasks produce relevant output
- Task description is updated

**Status: NOT IMPLEMENTED.** The coordinator does not currently auto-generate messages. This is listed as Phase 2 in the design doc.

---

## 6. Gaps and Issues

### Critical Gaps

1. **No mechanism to block `wg done` if messages are unread.** An agent can complete a task without ever checking messages. The `done.rs` command has no message-related checks. This means urgent messages (e.g., "STOP, the API changed") can be completely ignored.

2. **Quickstart and SKILL.md don't mention messaging.** New agents (and the top-level orchestrator) are never told that `wg msg` commands exist. The quickstart (`src/commands/quickstart.rs`) covers every other major feature but omits messaging. The Claude Code skill (`SKILL.md`) similarly has no mention of `wg msg send/read/poll`. Agents only learn about messaging from the injected prompt section `MESSAGE_POLLING_SECTION`, which is a soft instruction.

3. **Running agents can't reliably receive messages.** The wrapper script polling loop writes messages to the output log, but `claude --print` doesn't re-read its output. The agent only sees messages if it happens to run `wg msg read` itself. This is acknowledged in the design doc as a v1 limitation.

4. **No cursor advancement at spawn time in code.** The `build_scope_context()` function in `context.rs:132-134` formats queued messages for the prompt and has a comment saying "cursor advancement happens after spawn in execution.rs, where the agent_id is known." However, I don't see the cursor actually being advanced in `execution.rs`. The integration test (`tests/integration_messaging.rs:482-508`) manually simulates cursor advancement, suggesting it may be a gap.

5. **`$WG_MSG_FILE` not set.** The design doc says the shell adapter sets this env var, but it's not in the spawn code.

### Moderate Gaps

6. **No `wg msg gc` command.** The design doc specifies garbage collection for old message files, but it doesn't exist.

7. **No message threading or reply-to.** Messages are flat — no way to reply to a specific message or create conversation threads.

8. **No message limit/pagination.** The design doc mentions `--limit N` for large queues, but it's not implemented.

9. **TUI SendMessage action scaffolded but not wired.** `src/tui/viz_viewer/state.rs:90` defines `TextPromptAction::SendMessage(String)` and `event.rs:196` and `render.rs:1149` handle the enum variant, but the self-hosting report notes it's "planned but not wired."

10. **Coordinator doesn't auto-send messages.** Design doc Phase 2 items (auto-messages when dependencies complete, etc.) are not implemented.

### Minor Gaps

11. **No rate limiting on message sends.** A runaway agent could flood a task's queue.

12. **No message size limits.** Messages can be arbitrarily large, which could break the JSONL append atomicity guarantee (>PIPE_BUF).

13. **No encryption for cross-repo messages.** Federation messages travel in plaintext.

---

## 7. End-to-End Message Flow

### Pre-Spawn (Messages Queued Before Task is Claimed)

```
User/Coordinator → wg msg send <task-id> "text"
                        │
                        ▼
              .wg/messages/{task-id}.jsonl  (append JSONL line)
                        │
                        ▼  [at spawn time]
              build_scope_context() reads format_queued_messages()
                        │
                        ▼
              Queued messages injected into agent prompt
              under "## Queued Messages" header
```

### Mid-Execution (Messages to Running Agent)

```
User/Agent/Coordinator → wg msg send <task-id> "text"
     OR                  IPC SendMessage { task_id, body, ... }
                              │
                              ▼
                   .wg/messages/{task-id}.jsonl  (persistent storage)
                              │
                              ▼  [via deliver_message()]
                   MessageAdapter::deliver()
                              │
                              ▼
                   .wg/agents/{agent-id}/pending_messages.txt  (notification file)
                              │
                              ▼  [wrapper script poll_messages() every 10s]
                   pending_messages.txt → output.log  (moved atomically)
                   wg msg poll/read → output.log     (appended)
                              │
                              ▼
                   Agent's output.log contains messages
                   BUT claude --print doesn't re-read output
                              │
                              ▼
                   Agent self-polls with `wg msg read` (IF it does)
```

### Post-Completion

```
Messages persist in .wg/messages/{task-id}.jsonl
  - wg msg list still works
  - wg msg send still works (by design)
  - No cleanup mechanism exists (wg msg gc not implemented)
  - Cursor files persist in .wg/messages/.cursors/
```

---

## 8. Key Answers

| Question | Answer |
|----------|--------|
| Can agent A send a message to agent B's task and have B actually read it? | **Partially.** A can send to B's task ID. B will see it IF: (a) the message was sent before B spawned (included in prompt), or (b) B explicitly runs `wg msg read`. B is instructed to check via `MESSAGE_POLLING_SECTION` but not enforced. |
| What happens to unread messages when a task completes? | **Nothing.** Messages persist in the JSONL file. No warning, no blocking, no cleanup. |
| Is there any mechanism to block `wg done` if messages are unread? | **No.** `done.rs` has no message-related checks. |
| How are agents reminded to check messages? | Via `MESSAGE_POLLING_SECTION` injected into prompts at task+ scope. This is a soft instruction. The quickstart and SKILL.md don't mention messaging. |
| What does the message flow look like end-to-end? | See Section 7 above. Pre-spawn messages are reliably delivered via prompt injection. Mid-execution messages are best-effort via polling. |
