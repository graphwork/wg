# Design: User Board (.user-NAME) System

**Task:** design-user-boards
**Date:** 2026-03-28
**Status:** Design complete

---

## 1. Overview

User boards are per-user persistent conversation surfaces that live alongside coordinator tasks in the workgraph. They provide a human-owned message board that coordinators can subscribe to and interact with, persisting across coordinator lifecycles.

The core insight: a user board is **a task that represents a human's presence** in the graph. Just as `.coordinator-N` tasks represent running coordinator processes, `.user-NAME-N` tasks represent active human participants.

---

## 2. Design Decisions

### Decision 1: Identity

**Resolution:** The user handle is determined by the fallback chain: `WG_USER` env var → `USER` env var → `"unknown"` (reuses existing `current_user()` from `src/lib.rs:62`). A configured handle can be set via `wg config --user-handle <name>` for overriding the default.

**Task ID format:** `.user-{handle}-{N}` where N starts at 0 and auto-increments on archive.

Examples: `.user-erik-0`, `.user-alice-0`, `.user-erik-1` (after first archive).

**Rationale:** Matches the existing `.coordinator-{N}` pattern. The handle portion distinguishes users in multi-user scenarios (federation, shared repos). The numeric suffix enables archival history while keeping the active board predictable.

**Convenience alias:** `wg msg send .user-erik "..."` resolves `.user-erik` to the highest active `.user-erik-N`. This means users rarely type the suffix.

### Decision 2: Lifecycle

**Resolution:** Auto-created on first interaction via **lazy initialization**:

1. `wg msg send .user-NAME "..."` — if no active `.user-NAME-N` exists, creates `.user-NAME-0` automatically.
2. Coordinator startup — the coordinator checks if a board exists for `current_user()`. If not, creates one.
3. `wg user init [NAME]` — explicit creation for users who want to set up before first message.

**Archival:** `wg done .user-NAME` (or `wg user archive [NAME]`):
- Marks current `.user-NAME-N` as Done with `archived` tag.
- Creates `.user-NAME-{N+1}` with status `InProgress`.
- Messages from the old board are NOT migrated — they remain accessible via `wg msg list .user-NAME-N`.

**No auto-archive.** Users control when to archive. Boards are long-lived by default.

### Decision 3: Task Properties

**Resolution:** User boards are system tasks with the following properties:

| Property | Value | Rationale |
|----------|-------|-----------|
| ID prefix | `.user-` | Dot-prefix = system task, filtered from `wg list` by default |
| Status | `InProgress` (while active) | Externally-linked process pattern, same as coordinators |
| Tag | `user-board` | Discoverable via tag filter, analogous to `coordinator-loop` |
| Assigned | `None` | Human-owned, not assigned to any agent |
| Agent | `None` | No agency identity — this is the human |
| CycleConfig | `None` | No cycle behavior — archival creates a new task explicitly |
| Description | `"User board for {handle} — persistent conversation surface."` | Standard description |

**Not a new type.** User boards are regular `Task` nodes with a conventional ID prefix and tag. No schema changes to `graph.rs` are needed. This follows the coordinator precedent where `.coordinator-N` tasks are just tasks with `coordinator-loop` tags.

### Decision 4: Storage

**Resolution:** Reuse existing message infrastructure entirely.

- **Message storage:** `.workgraph/messages/.user-{handle}-{N}.jsonl` — standard JSONL message file with flock-based append.
- **Read cursors:** `.workgraph/messages/.cursors/{agent-id}.{.user-handle-N}` — standard cursor files.
- **Task log:** Embedded in the task's `log` field in `graph.jsonl`.

**Compaction:** User boards get independent compaction via the existing compactor. When a board accumulates enough messages (threshold configurable via `wg config --user-board-compact-threshold N`, default 100), the compactor summarizes old messages into a context note, same as coordinator compaction. This is a **phase 2** feature — initial implementation stores messages without compaction.

**No new storage paths or formats.** Everything fits in the existing message and task log infrastructure.

### Decision 5: Visibility

**Resolution:** User boards are shown by default in the TUI alongside coordinators.

- **Graph view (left panel):** User board tasks appear in the graph like any other task. They use **yellow** node coloring (coordinators use cyan/blue).
- **Tab bar (right panel):** User boards appear in a **separate tab bar row** below the coordinator tab bar. Yellow-colored tabs with the user handle as label (e.g., `● erik`). Active by default.
- **`wg list`:** Hidden by default (dot-prefix filtering). Shown with `wg list --all` or `wg list --tag user-board`.
- **`wg viz`:** Included in the graph visualization. Yellow node coloring.

**Rationale for separate tab bar row:** Coordinator tabs and user board tabs serve different purposes. Coordinators are AI agents you chat with; user boards are human presence markers. Mixing them in one row creates confusion. Two rows is clean and scalable.

### Decision 6: CLI Interface

**Resolution:** Primarily reuse existing commands. One new convenience command.

**Existing commands that work today (or with minimal changes):**

| Command | Behavior | Changes needed |
|---------|----------|----------------|
| `wg msg send .user-erik "hello"` | Send message to user board | Auto-create board if missing (lazy init) |
| `wg msg list .user-erik` | List messages on board | Resolve alias to active `.user-erik-N` |
| `wg msg read .user-erik` | Read unread messages | Resolve alias |
| `wg show .user-erik` | Show board task details | Resolve alias |
| `wg done .user-erik` | Archive board, create N+1 | Add archival + successor logic |
| `wg log .user-erik "note"` | Add log entry | Resolve alias |

**New commands:**

| Command | Behavior |
|---------|----------|
| `wg user init [NAME]` | Explicitly create a user board. NAME defaults to `current_user()`. Idempotent — if board exists, prints its ID. |
| `wg user list` | List all user boards (active and archived). |
| `wg user archive [NAME]` | Sugar for `wg done .user-NAME` with the auto-increment behavior. |

**Alias resolution:** The key UX improvement. When a command receives `.user-erik` (no numeric suffix), it resolves to the highest-numbered active (non-archived, non-Done) `.user-erik-N`. This is implemented as a utility function in `src/commands/mod.rs` or a method on `WorkGraph`.

```rust
/// Resolve a user board alias like `.user-erik` to the active `.user-erik-N`.
/// Returns the original ID if it's already fully qualified or not a user board.
fn resolve_user_board_alias(graph: &WorkGraph, id: &str) -> String {
    // Only resolve if it matches `.user-{handle}` without a trailing `-N`
    if !id.starts_with(".user-") { return id.to_string(); }
    let suffix = &id[".user-".len()..];
    // If suffix already ends with -N, it's fully qualified
    if suffix.rsplit('-').next().map_or(false, |s| s.parse::<u32>().is_ok()) {
        return id.to_string();
    }
    // Find highest active .user-{handle}-N
    let prefix = format!("{}-", id);
    graph.tasks()
        .filter(|t| t.id.starts_with(&prefix))
        .filter(|t| !t.status.is_terminal())
        .filter_map(|t| {
            t.id.rsplit('-').next()
                .and_then(|n| n.parse::<u32>().ok())
                .map(|n| (n, t.id.clone()))
        })
        .max_by_key(|(n, _)| *n)
        .map(|(_, id)| id)
        .unwrap_or_else(|| id.to_string())
}
```

### Decision 7: TUI Interface

**Resolution:** User boards appear in the Chat tab area with their own visual treatment.

**Tab bar layout:**
```
┌─────────────────────────────────────────────┐
│ ● C0: Main  ● C1: Debug  [+]               │  ← Coordinator tabs (cyan dots)
│ ● erik  ● alice  [+]                        │  ← User board tabs (yellow dots)
├─────────────────────────────────────────────┤
│                                             │
│  Messages rendered here                     │
│  (from .user-erik-0's message queue)        │
│                                             │
├─────────────────────────────────────────────┤
│ > type message here                         │
└─────────────────────────────────────────────┘
```

**Tab appearance:**
- Yellow dot color (●) — distinct from coordinator cyan dots
- Label: user handle (e.g., `erik`) — not the full task ID
- Active tab: bold + yellow underline
- State indicator: `●` (active), `○` (archived/done)

**Behavior:**
- Clicking a user board tab switches the message area to show that board's messages.
- The `[+]` button opens a text prompt for creating a new user board (like coordinator `[+]`).
- Left/Right arrow keys cycle between user board tabs when user board tab bar is focused.
- Messages are loaded from `.workgraph/messages/.user-{handle}-{N}.jsonl` via the same polling mechanism used for coordinator chat (`poll_chat_messages` pattern).

**Chat input routing:**
- When a user board tab is active, typed messages go to `wg msg send .user-{handle}-{N} "..."`.
- This is a message queue, not a coordinator chat — there's no "awaiting response" state.
- Messages from agents/coordinators appear as incoming messages with sender attribution.

**Implementation note:** The TUI already has the coordinator tab bar infrastructure (`CoordinatorTabHit`, `CoordinatorPlusHit`, `list_coordinator_ids_and_labels`). The user board tab bar is a parallel structure: `UserBoardTabHit`, `UserBoardPlusHit`, `list_user_board_ids_and_labels`. Same rendering logic, different data source (filter by `user-board` tag instead of `coordinator-loop` tag).

### Decision 8: Relationship to Coordinators

**Resolution:** Independent with subscription-based interaction.

**Independence:**
- User boards exist in the graph independently of any coordinator.
- A user board persists across coordinator start/stop/archive cycles.
- Multiple coordinators can interact with the same user board.

**Interaction model:**
- Coordinators discover user boards by scanning for tasks tagged `user-board` with non-terminal status.
- When a coordinator sees a new message on a user board (via the existing message polling infrastructure), it can:
  1. Read the message (`wg msg read .user-erik`)
  2. Reply to the board (`wg msg send .user-erik "response"`)
  3. Create tasks in response to user requests

**No explicit subscription mechanism in phase 1.** Coordinators simply poll user board message queues as part of their tick loop. This leverages the existing "resurrection" pattern where messages to Done/InProgress tasks trigger coordinator attention.

**Future extension:** Explicit subscription via `wg subscribe .coordinator-0 .user-erik` for selective routing. Not needed initially since the coordinator already scans all tasks.

### Decision 9: Crypto Identity

**Future path (NOT implemented now):**

The user handle is currently `$USER` / `WG_USER` — a local convention with no verification. The extension point for cryptographic identity:

1. **Key generation:** `wg identity init` generates an Ed25519 keypair, stores private key in `~/.config/workgraph/identity.key` (user-local, never in repo).
2. **Public key hash:** The SHA-256 hash of the public key becomes the user's **verified identity**. Stored in `.workgraph/identities/{hash}.pub`.
3. **Message signing:** Each message gets an optional `signature` field (Ed25519 over `{timestamp}:{body}`). Unsigned messages are still valid but marked as unverified.
4. **Board binding:** A user board's `agent` field (currently None) would hold the identity hash, proving board ownership.
5. **Federation:** Identity hashes are globally unique — the same user across federated graphs is recognized by key, not by handle.

**Extension points in this design:**
- `Task.agent` field (exists, currently None for user boards) — will hold identity hash
- `Message` struct has room for a `signature: Option<String>` field
- Config has room for `identity_key_path: Option<String>`

**No code changes needed now.** The design accommodates this by not hardcoding any identity assumptions beyond "handle is a string."

---

## 3. Data Model Changes

**No schema changes required.** User boards are conventional Task nodes. The design relies on:

| Convention | How enforced |
|------------|-------------|
| ID prefix `.user-` | `is_user_board()` helper function (new, mirrors `is_system_task()`) |
| Tag `user-board` | Applied on creation |
| Status `InProgress` | Set on creation, maintained while active |
| No agent assignment | Left as None |

**New helper function in `src/graph.rs`:**
```rust
/// Returns `true` if the task ID represents a user board.
pub fn is_user_board(task_id: &str) -> bool {
    task_id.starts_with(".user-")
}
```

---

## 4. Files to Modify (with Scope)

### Phase 1: Core (MVP — get user boards working)

| File | Scope | Estimate |
|------|-------|----------|
| `src/graph.rs` | Add `is_user_board()` helper (~3 lines) | S |
| `src/commands/mod.rs` | Add `resolve_user_board_alias()` utility. Wire into task ID resolution for msg, show, done, log commands. | M |
| `src/commands/msg.rs` | In `run_send()`: if task doesn't exist and ID matches `.user-*`, auto-create the board task (lazy init). | M |
| `src/main.rs` / `src/cli.rs` | Add `wg user` subcommand with `init`, `list`, `archive` sub-subcommands. | M |
| `src/commands/user.rs` | **New file.** Implements `user init`, `user list`, `user archive`. | M |
| `src/commands/done.rs` | In done handler: if task has `user-board` tag, auto-create successor `.user-NAME-{N+1}`. | M |
| `src/commands/service/coordinator.rs` | In coordinator startup/tick: check for user board existence, create if missing. | S |

**Phase 1 total: ~7 files, ~250-350 lines of new/modified code.**

### Phase 2: TUI Integration

| File | Scope | Estimate |
|------|-------|----------|
| `src/tui/viz_viewer/state.rs` | Add `UserBoardTabHit`, `UserBoardPlusHit` structs. Add `list_user_board_ids_and_labels()`, `switch_user_board()`, `active_user_board_id` field. User board chat state in `user_board_chats: HashMap<String, ChatState>`. | L |
| `src/tui/viz_viewer/render.rs` | Render user board tab bar row below coordinator tabs. Yellow dot coloring. Message area rendering when user board tab is active. | L |
| `src/tui/viz_viewer/event.rs` | Key handlers for user board tab cycling, [+] button, click support on user board tab bar. | M |
| `src/commands/viz/mod.rs` | Yellow coloring for `.user-*` nodes in ASCII viz. | S |
| `src/tui/viz_viewer/screen_dump.rs` | Include active user board in screen dump state. | S |

**Phase 2 total: ~5 files, ~400-500 lines of new/modified code.**

### Phase 3: Polish & Extensions

| File | Scope | Estimate |
|------|-------|----------|
| `src/config.rs` | Add `user_handle: Option<String>`, `user_board_compact_threshold: Option<u32>` to config. | S |
| `src/service/compactor.rs` | User board compaction support (summarize old messages). | M |
| `src/commands/quickstart.rs` | Mention user boards in quickstart output. | S |
| `docs/AGENT-GUIDE.md` | Document user board interaction for agents. | S |

**Phase 3 total: ~4 files, ~100-150 lines.**

---

## 5. CLI Command Surface Summary

### New top-level subcommand: `wg user`

```
wg user init [NAME]        # Create user board (NAME defaults to current_user())
wg user list               # List all user boards (active + archived)
wg user archive [NAME]     # Archive active board, create successor
```

### Modified existing commands (alias resolution)

```
wg msg send .user-erik "msg"   # Resolves to active .user-erik-N, auto-creates if needed
wg msg list .user-erik         # Resolves alias
wg msg read .user-erik         # Resolves alias
wg show .user-erik             # Resolves alias
wg done .user-erik             # Archives + creates successor
wg log .user-erik "note"       # Resolves alias
```

---

## 6. TUI Rendering Spec

### Tab Bar

- **Position:** Below coordinator tab bar, above message area
- **Color:** Yellow dots (●) for user boards vs cyan dots for coordinators
- **Label format:** `● {handle}` (e.g., `● erik`)
- **Active tab:** Bold, yellow background highlight
- **Buttons:** `[+]` to create new board, `[×]` on hover/active to archive
- **Overflow:** Same ellipsis behavior as coordinator tabs (`… [+]`)

### Message Area

- **When user board tab is active:** Renders messages from the board's JSONL file
- **Message format:** `{sender} ({timestamp}): {body}` — same as coordinator chat messages
- **Input area:** Standard text input, sends via `wg msg send .user-{handle}-N`
- **No "awaiting response" spinner** — user boards are async message queues, not synchronous chat

### Graph View Coloring

- User board nodes: Yellow foreground
- Coordinator nodes: Cyan foreground (existing)
- Regular task nodes: White/default (existing)

---

## 7. Phasing Recommendation

### Phase 1: Core Infrastructure (build first)
**Goal:** User boards exist, messages work, CLI is functional.

1. `is_user_board()` helper in graph.rs
2. `resolve_user_board_alias()` utility
3. Lazy init in `wg msg send`
4. `wg user init/list/archive` commands
5. Archive-with-successor logic in `wg done`
6. Coordinator awareness (create board on startup)

**Validates:** All 9 design decisions at the CLI level. Users can create boards, send messages, and archive.

### Phase 2: TUI Integration (build second)
**Goal:** User boards are visible and interactive in the TUI.

1. User board tab bar rendering
2. Tab switching and message display
3. Input routing to user board message queue
4. Graph view yellow coloring
5. Click and keyboard navigation

**Validates:** Visual presence, message flow in TUI.

### Phase 3: Polish (build last)
**Goal:** Production quality.

1. Config options (handle override, compaction threshold)
2. User board compaction
3. Documentation updates
4. Quickstart integration

---

## 8. Open Questions (Resolved)

| Question | Resolution |
|----------|-----------|
| New task type vs convention? | **Convention** — same as coordinators. Tag-based discovery. |
| Separate storage vs reuse? | **Reuse** — existing message JSONL + task log infrastructure. |
| Auto-create vs explicit? | **Both** — lazy init on first message + explicit `wg user init`. |
| Tab bar: mixed vs separate? | **Separate** row from coordinators for clarity. |
| Archival: auto vs manual? | **Manual** — users control when to archive. |
| Compaction: yes/no? | **Phase 2** — not in MVP, uses existing compactor when ready. |
