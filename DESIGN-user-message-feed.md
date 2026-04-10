# Design: Humane User Message Collection in the TUI

## Context

This document addresses the message collection UX in the workgraph TUI, specifically for coordinator tasks that collect user/agent messages via a feed-like interface.

**Related**: `design-coordinator-lifecycle` (sibling) covers coordinator state management. This document focuses on the **message data layer** — storage, archival, and display of messages collected by coordinator tasks.

---

## Current Problems (Articulated Precisely)

### Problem 1: Messages lack a recipient field

When the coordinator sends a message to a task (via `wg msg send`), the message is stored with only `sender` — there is no `recipient` field. This makes it impossible to audit "who was told what" because the direction is inferred from sender logic rather than explicit declaration.

**Where**: `src/messages.rs` `Message` struct (line 41)

**Current fields**: `id`, `timestamp`, `sender`, `body`, `priority`, `status`, `read_at`

**Missing**: `recipient`

### Problem 2: Archiving a task destroys its message history

When `wg archive` runs on a task:
1. The task is removed from `graph.jsonl` and appended to `archive.jsonl`
2. The task's message file (`.workgraph/messages/{task-id}.jsonl`) is **silently orphaned and lost**

This is a data-resilience failure. The archive contains only the Task node, not its message history.

**Where**: `src/commands/archive.rs` `run()` function (line ~170)

### Problem 3: The TUI messages panel shows raw format

The `MessagesPanelState::rendered_lines` (used for fallback display) shows:
```
[2m ago] user: Hello agent
```

The structured `MessageEntry` has more information (direction, delivery status, read_at) but the default fallback rendering only shows `sender: body`.

**Where**: `src/tui/viz_viewer/state.rs` line 7793

---

## Design: Data Model Changes

### Change 1: Add `recipient` to `Message`

```rust
// src/messages.rs line 41
pub struct Message {
    pub id: u64,
    pub timestamp: String,           // ISO 8601
    pub sender: String,             // "user", "coordinator", agent-id, task-id
    pub recipient: String,          // NEW: task-id this message is directed to
    pub body: String,
    pub priority: String,           // "normal" | "urgent"
    pub status: DeliveryStatus,
    pub read_at: Option<String>,     // ISO 8601 when agent read this
}
```

**Validation**: The `send_message()` function takes `task_id` as parameter (the queue destination) — this same value should be stored as `recipient`.

### Change 2: Archive task messages alongside the task

When a task is archived, its message file should be **moved** to an archive location, not deleted:

```
Archive structure:
.workgraph/
  archive/
    tasks/               # Existing: archived Task nodes
      archive.jsonl
    messages/           # NEW: archived message files
      {task-id}.jsonl
```

**Implementation in `src/commands/archive.rs`**:
- Add `messages_archive_dir()` → `dir.join("archive").join("messages")`
- When archiving a task, call `maybe_archive_messages(dir, task_id)` which:
  1. Checks if `.workgraph/messages/{task-id}.jsonl` exists
  2. If yes, move it to `.workgraph/archive/messages/{task-id}.jsonl`
  3. Create parent directories as needed
- When restoring a task, move messages back

### Change 3: Support reading archived messages

```rust
// src/messages.rs

/// Read messages from active OR archived location.
/// Tries active first, falls back to archive.
pub fn list_messages_anywhere(workgraph_dir: &Path, task_id: &str) -> Result<Vec<Message>> {
    let active = message_file(workgraph_dir, task_id);
    if active.exists() {
        return list_messages(workgraph_dir, task_id);
    }
    
    let archived = messages_archive_file(workgraph_dir, task_id);
    if archived.exists() {
        return list_messages_from_file(&archived);
    }
    
    Ok(vec![])
}

fn messages_archive_file(workgraph_dir: &Path, task_id: &str) -> PathBuf {
    workgraph_dir.join("archive").join("messages").join(format!("{}.jsonl", task_id))
}
```

---

## Design: TUI Changes

### Change 4: Richer message display in MessagesPanel

The `MessagesPanelState` already has `MessageEntry` with direction metadata. The issue is the **rendering** doesn't fully leverage it.

**Current render** (simplified from `render.rs`):
```
[2m ago] user: Hello agent
```

**Proposed render**:
```
┌─ you → design-user-message ────────────────────────────── 2m ago ─
│ Hello agent
│                                                              ✓✓
└─────────────────────────────────────────────────────────────────
```

Or more compactly:
```
  you → [task-id]: Hello agent                        2m ago  ✓✓
  agent → [task-id]: Working on it...                 1m ago  
```

**Key metadata to surface**:
1. **Sender → Recipient** with arrow indicator
2. **Timestamp** (already present, but more prominent)
3. **Delivery status**: `✓✓` (responded), `✓` (delivered), `⋯` (pending)
4. **Sequence number**: `msg #3` or similar
5. **Priority badge**: `[!]` for urgent (already present but should be more visible)

### Change 5: Archived task indicator in TUI

When selecting an archived task in the task list, the messages panel should:

1. Show a banner: `📦 Archived — message history preserved`
2. Display messages normally (since they're now in archive)
3. Disable the message input area (can't send to archived task)

### Change 6: Session context in messages

Add a `session_id` field to `Message` to group messages by coordinator session:

```rust
pub struct Message {
    // ... existing fields ...
    pub session_id: Option<String>,  // Coordinator session that created this message
}
```

This helps answer "was this message from the current session or a previous one?"

---

## Design: Archiving Behavior Matrix

| Action | Task Node | Message File | Can Resume Monitoring? |
|--------|-----------|--------------|------------------------|
| Archive task | Moved to `archive.jsonl` | Moved to `archive/messages/` | Yes — messages preserved |
| Restore task | Moved back to `graph.jsonl` | Moved back to `messages/` | Yes |
| Delete task (irreversible) | Gone | Gone | No |
| Task completes naturally | Stays in `graph.jsonl` | Stays in `messages/` | N/A |

**Principle**: Archiving is **transfer to cold storage**, not deletion. The data remains accessible.

---

## Design: Resume Monitoring Flow

### Scenario: User was monitoring `design-user-message`, then it got archived

1. User selects `design-user-message` in task list
2. TUI detects task is archived (has `archived` tag)
3. TUI loads messages from `archive/messages/design-user-message.jsonl`
4. Banner shown: `📦 This task is archived. Showing preserved message history.`
5. Message input disabled
6. User can scroll through all historical messages

### Scenario: User restores an archived task

1. User runs `wg archive restore design-user-message`
2. System:
   - Moves task node back to `graph.jsonl`
   - Moves message file back to `messages/`
   - Removes `archived` tag
3. Task reappears in active task list
4. Message monitoring resumes normally

---

## Design: API Changes for Coordinators

The coordinator service (`src/commands/service/coordinator.rs`) sends messages to tasks via `deliver_message()`. After the data model changes:

```rust
// No API signature changes needed — recipient is derived from task context
// But the Message struct change propagates through:
// - deliver_message() creates Message with recipient set
// - list_messages() returns Message with recipient visible
```

---

## Implementation Phases

### Phase 1: Data Model (Low Risk)
- [ ] Add `recipient` field to `Message` struct
- [ ] Update `send_message()` to populate `recipient`
- [ ] Add `session_id` field (optional, can defer)

### Phase 2: Archival Resilience (Medium Risk)
- [ ] Create `archive/messages/` directory structure
- [ ] Add `maybe_archive_messages()` to move messages on task archive
- [ ] Add `maybe_restore_messages()` to move messages on task restore
- [ ] Add `list_messages_anywhere()` to read from either location
- [ ] Update TUI to call `list_messages_anywhere()`

### Phase 3: UI Improvements (Low Risk)
- [ ] Update message panel rendering to show sender→recipient
- [ ] Add archived task banner
- [ ] Disable input for archived task's message panel
- [ ] Surface delivery status more prominently

### Phase 4: Testing & Validation
- [ ] Add unit tests for archive/restore message preservation
- [ ] Add integration test: archive task → restore → verify messages
- [ ] Manual verification in TUI

---

## Files to Modify

| File | Changes |
|------|---------|
| `src/messages.rs` | Add `recipient` field, add `list_messages_anywhere()` |
| `src/commands/archive.rs` | Add message file move on archive/restore |
| `src/tui/viz_viewer/state.rs` | Update `load_messages_panel()` to use new API |
| `src/tui/viz_viewer/render.rs` | Update message rendering to show richer metadata |
| `src/commands/service/coordinator.rs` | Minor: ensure `deliver_message` passes task_id correctly |

---

## Out of Scope

- **Chat inbox/outbox** (`src/chat.rs`) — already has proper archival via rotation. This design is specifically about **task message queues** (`src/messages.rs`).
- **Message search** — searching archived messages is a future enhancement.
- **Cross-task message threads** — messages are per-task, no threading across tasks.

---

## Success Criteria

1. **Data resilience**: Archiving a task does NOT lose message data. Data is in `archive/messages/{task-id}.jsonl`.
2. **Full auditability**: Every message shows sender, recipient, timestamp, and sequence.
3. **Seamless resume**: Monitoring can be resumed on an archived task without data loss.
4. **Clear UI signals**: Users can tell if a task is archived, and where their messages went.
