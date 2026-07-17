# Bug Report: User Board Messages Leaking into Coordinator Chat

## Root Cause

The bug has **two contributing factors** that work together:

### Factor 1: User Board Context Injection into Every Coordinator (the primary cause)

**File:** `src/commands/service/coordinator_agent.rs:2550-2578`

The function `build_coordinator_context()` unconditionally injects the last 10 user board messages into **every coordinator's** system context update. This happens on every coordinator turn:

```rust
// --- User Board Context ---
{
    use workgraph::graph::resolve_user_board_alias;
    let handle = workgraph::current_user();
    let alias = format!(".user-{}", handle);
    let resolved = resolve_user_board_alias(&graph, &alias);
    if resolved != alias {
        // Include recent messages from the user board
        if let Ok(msgs) = workgraph::messages::list_messages(dir, &resolved) {
            let recent: Vec<_> = msgs.iter().rev().take(10).collect();
            // ... formats and pushes into parts
        }
    }
}
```

This context gets prepended to the user's message at line 670:
```rust
format!("{}\n\n---\n\nUser message:\n{}", context, req.message)
```

So every time a coordinator processes a chat message, it sees the user board's message history embedded in its input. When the coordinator then responds, the response is recorded in the chat log — but the input (which contained user board messages) is also visible in the chat log/view, making it look like user board messages are "in" the coordinator chat.

### Factor 2: Chat Messages Are Forwarded to the User Board

**Files:**
- `src/commands/service/coordinator.rs:3351-3352` (non-agent path)
- `src/commands/service/mod.rs:1134-1135` (agent path)

Every chat message from the user is forwarded to the user board via `forward_chat_to_user_board()`. The forwarded message is prefixed with routing context:

```rust
let routed_content = format!("user [coord:{}]: {}", coordinator_id, content);
```

This creates a **feedback loop**:
1. User sends message to coordinator chat
2. Message gets forwarded to user board as `user [coord:N]: <content>`
3. Next time any coordinator builds its context, it reads those messages back from the user board
4. The coordinator's context now contains `user [coord:15]: ...` entries — exactly the symptom described

### Why It's Worse When Opening a New Coordinator

When a new coordinator was created (via the explicit New-chat control, or historically via the now-removed `ensure_user_coordinator` bootstrap), it called `build_coordinator_context()` on its first turn. At that point, the user board may have accumulated messages from all previous coordinator interactions (since `forward_chat_to_user_board` has been running). The new coordinator sees **all** of those historical messages in its context, making it look like those messages were sent to it.

## Message Flow Diagram

```
User types in Coordinator N chat
  │
  ├──> inbox.jsonl (coord N)
  │     │
  │     ├──> route_chat_to_agent() / process_chat_inbox_for()
  │     │     │
  │     │     ├──> forward_chat_to_user_board()
  │     │     │     │
  │     │     │     └──> .user-erik-0 messages (as "user [coord:N]: ...")
  │     │     │
  │     │     └──> coordinator agent stdin (with context)
  │     │
  │     └──> build_coordinator_context() reads .user-erik-0 messages
  │           │
  │           └──> "### User Board" section injected into ALL coordinators
  │
  └──> TUI chat view shows the coordinator's conversation,
       which now contains user board messages in context blocks
```

## Proposed Fix

### Option A: Remove User Board Context from Coordinator (recommended)

**File:** `src/commands/service/coordinator_agent.rs`
**Change:** Remove the "User Board Context" section (lines 2550-2578) from `build_coordinator_context()`.

**Rationale:** The coordinator should not need to see user board messages. The user board is a separate communication channel for asynchronous, cross-coordinator messaging. The coordinator already receives the user's message directly via the inbox — it doesn't need a rehash of historical messages from a different channel.

If the intent was to give the coordinator awareness of the user's overall activity, this should be opt-in and clearly separated, not injected into every turn's context.

### Option B: Filter Out Coordinator-Sourced Messages (partial fix)

**File:** `src/commands/service/coordinator_agent.rs:2558`
**Change:** When reading user board messages, filter out those that originated from coordinator chat forwarding (messages matching `user [coord:N]:` prefix).

This would prevent the feedback loop but still show genuinely external user board messages. However, this is fragile and doesn't address the core design issue that coordinator context shouldn't include user board content.

### Option C: Stop Forwarding Chat to User Board (alternative)

**Files:** `src/commands/service/coordinator.rs:3352` and `src/commands/service/mod.rs:1135`
**Change:** Remove the `forward_chat_to_user_board()` calls.

This breaks the feedback loop but also removes a feature — the user board won't capture coordinator chat history. This may or may not be desirable depending on the intended purpose of the user board.

### Recommended Approach: Option A + Keep Forwarding

1. **Remove** the "User Board Context" block from `build_coordinator_context()` (lines 2550-2578 in `coordinator_agent.rs`)
2. **Keep** `forward_chat_to_user_board()` in both call sites — the user board continues to capture chat history as an audit trail
3. **Result:** Coordinators no longer see user board messages in their context; user board continues to accumulate chat history for the user's reference

### Files to Modify

| File | Change |
|------|--------|
| `src/commands/service/coordinator_agent.rs` | Delete lines 2550-2578 (User Board Context section) |

### Tests to Update

Existing tests for `build_coordinator_context` that verify user board context inclusion should be updated or removed:

| File | Test |
|------|------|
| `src/commands/service/coordinator_agent.rs` | Check for `test_build_coordinator_context_user_board` or similar |

## What Goes Where

| Content | Coordinator Chat | User Board |
|---------|-----------------|------------|
| User's direct messages to coordinator | Yes (inbox) | Yes (forwarded copy) |
| Coordinator's responses | Yes (outbox) | No |
| Cross-coordinator user messages | No | Yes |
| `wg msg send .user-erik "..."` | No | Yes |
| Graph events / task updates | Yes (context) | No |
| Other coordinators' conversations | No | No |
