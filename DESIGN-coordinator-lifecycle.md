# Design: Humane Coordinator Lifecycle in the TUI

## Context

This document addresses the coordinator lifecycle UX in the wg TUI — specifically, the problems that arise when users try to create new coordinators or resume existing ones, and the system behaves unexpectedly by resurrecting archived state.

**Related**: `design-user-message-feed` (sibling) covers message storage alongside coordinator tasks. This document focuses on **coordinator state management** — the lifecycle of coordinator tasks, the distinction between active/archived/dormant states, and the TUI's handling of these transitions.

---

## Current Problems (Articulated Precisely)

### Problem 1: Archived coordinator state bleeds through after archival

**Symptom**: User runs `wg archive <coordinator-id>` or uses the TUI archive action. The coordinator's task status changes to `Done` and it gains the `archived` tag. But when the user switches back to that coordinator tab, the chat panel still shows old messages and state — as if the coordinator were still alive.

**Root cause**: `switch_coordinator(target_id)` in `state.rs` loads `coordinator_chats.remove(&target_id)` from `coordinator_chats` HashMap. But the `coordinator_chats` HashMap is a **in-memory cache** of chat state, not a view of coordinator aliveness. If the user had been viewing coordinator-1 before it was archived, the old chat state remains in memory. Switching to coordinator-1 after archival retrieves that stale in-memory state instead of detecting the archival and showing a clear message.

**Location**: `src/tui/viz_viewer/state.rs` — `switch_coordinator()` method

### Problem 2: No distinction between "create fresh coordinator" and "resume coordinator"

**Symptom**: User presses `+` to create a new coordinator. The system creates one — but if the user actually wanted to **resume** a dormant coordinator (archived but still in graph), the new-coordinator flow doesn't ask. And if the user wanted a **truly fresh start** (new ID with blank slate), the system might redirect them to an old coordinator-1 with all its accumulated state.

**Root cause**: The `TextPromptAction::CreateCoordinator` dialog only prompts for an optional name. It has no mechanism to distinguish:
- "Give me a brand new coordinator with a fresh ID and clean slate"
- "Let me pick which coordinator to resume"

The "Fresh Start" intent and "Resume" intent are conflated.

**Location**: `src/tui/viz_viewer/event.rs` — `CreateCoordinator` handling; `src/tui/viz_viewer/render.rs` — dialog title

### Problem 3: ID assignment picks dead IDs instead of fresh ones

**Symptom**: User creates coordinator → gets ID 0. Archives it. Creates another → gets ID 0 again (resurrects). User expected ID 1.

**Root cause**: The auto-naming logic in `is_coordinator_slot_available()` (ipc.rs:1141–1160) skips InProgress coordinators but treats **archived and abandoned coordinators as occupied slots** (`return false` in both cases). The ID counter then increments and finds the next free slot. This is actually correct behavior — it doesn't resurrect. But the problem is the user has no UI indication that coordinator-0 is archived and they're getting coordinator-1, nor can they explicitly "resume" coordinator-0.

Wait — re-reading the problem statement: "User tries to create a new coordinator → system redirects to archived coordinator-1 (resurrects old state)". This suggests the system IS resurrecting. Let me re-examine.

Looking at `is_coordinator_slot_available`:
```rust
// Archived or abandoned — skip this slot, treat as occupied
return false;
```

This means archived slots are SKIPPED (treated as taken), not resurrected. So the counter moves to the next free slot. This seems correct for "don't reuse IDs". But the problem description says resurrection IS happening.

Possibility 1: The `switch_coordinator` path resurrects in-memory state from `coordinator_chats`.
Possibility 2: The `list_coordinator_ids` path shows archived coordinators as available.
Possibility 3: The `ensure_user_coordinator` path finds archived coordinators and switches to them.

Looking at `list_coordinator_ids_and_labels`:
```rust
.filter(|t| !matches!(t.status, Status::Abandoned))
.filter(|t| !t.tags.iter().any(|tag| tag == "archived"))
```

Good — archived/abandoned are filtered out.

Looking at `ensure_user_coordinator`:
```rust
.filter(|t| !matches!(t.status, workgraph::graph::Status::Abandoned))
.filter(|t| !t.tags.iter().any(|tag| tag == "archived"))
```

Good — also filtered.

So the resurrection is likely in-memory state via `coordinator_chats`. The archived coordinator task itself isn't being resurrected from the graph — the user's VIEW of it (chat history) is being preserved in memory and shown when they switch back.

### Problem 4: No clear state indicators in the coordinator tab bar

**Symptom**: The tab bar shows `coord:0`, `coord:1`, etc. with no visual indication of which are active, archived, or dormant. User cannot tell at a glance which coordinator they're looking at.

**Root cause**: `CoordinatorTabHit` contains only layout info, not coordinator state. The renderer doesn't color-code or badge tabs by state.

**Location**: `src/tui/viz_viewer/render.rs` — coordinator tab rendering

---

## Design: State Model

### Coordinator States

A coordinator task (`.coordinator-N`) can be in one of three lifecycle states:

| State | Task Status | Tags | Can Receive Messages? | Shown in Tab Bar? | Fresh Start Candidate? |
|-------|-------------|------|----------------------|-------------------|------------------------|
| **Active** | `InProgress` | `coordinator-loop` | Yes | Yes | N/A |
| **Archived** | `Done` | `archived` | No | No (filtered) | No — stays buried |
| **Abandoned** | `Abandoned` | (none) | No | No (filtered) | No — stays buried |

**Key principle**: Archival is a **terminal cold-storage state**, not a paused state. An archived coordinator's data (chat history, task state) is preserved but not resumable through the normal "switch to coordinator" flow. The user must explicitly use a "Resume Coordinator" action to re-activate it.

### State Transitions

```
[Create] → Active (InProgress, coordinator-loop)
[Archive action] → Archived (Done, archived tag added)
[Resume action] → Active (status → InProgress, archived tag removed)
[Delete action] → Abandoned (Abandoned status)
```

**Critical**: When resuming an archived coordinator:
1. Remove the `archived` tag
2. Change status from `Done` → `InProgress`
3. Keep all existing chat history (it's preserved, not discarded)
4. The coordinator is now "live" again with its accumulated context

**Critical**: When user wants a "Fresh Start":
1. Create a NEW coordinator with a new ID (never reuse an archived ID)
2. The old archived coordinator stays archived
3. New coordinator has no inherited state

---

## Design: TUI Changes

### Change 1: Add "Fresh Start" vs "Resume" Choice Dialog

When the user presses `+` to create a coordinator, instead of immediately prompting for a name, show a choice dialog:

```
┌─ New Coordinator ──────────────────────────────────────────────────────┐
│                                                                         │
│   What would you like to do?                                            │
│                                                                         │
│   [N] Start fresh — create a brand new coordinator with a clean slate  │
│   [R] Resume — pick an archived coordinator to reactivate               │
│   [C] Cancel                                                            │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

**Implementation**:
- Add a new `ChoiceDialogAction::NewOrResumeCoordinator`
- The `+` key binding changes from `InputMode::TextPrompt(TextPromptAction::CreateCoordinator)` to `InputMode::Choice(ChoiceDialogAction::NewOrResumeCoordinator)`
- "Start fresh" → proceed to name prompt, then `create_coordinator()`
- "Resume" → show a list-picker of archived coordinators
- "Cancel" → dismiss

**Rationale**: This directly addresses the conflation of "fresh start" and "resume" intents. The user must consciously choose.

### Change 2: Archival Detection in switch_coordinator

When `switch_coordinator(target_id)` is called, before loading chat state:

```rust
// Check if the target coordinator is archived
let coord_task_id = format!(".coordinator-{}", target_id);
if let Some(task) = graph.get_task(&coord_task_id) {
    if task.tags.iter().any(|tag| tag == "archived") {
        // Show archival banner instead of chat
        self.show_archival_banner = true;
        self.chat = ChatState::default(); // Don't load old in-memory state
        return;
    }
}
self.show_archival_banner = false;
// ... existing chat loading logic
```

**Key**: Don't load `coordinator_chats.remove(&target_id)` for archived coordinators. The archived coordinator's chat history is preserved in the `coordinator_chats` HashMap (in-memory) but we should NOT show it when switching to an archived coordinator — instead show the archival banner.

Wait — but the archival banner should show the PRESERVED message history (from `coordinator_chats`), not discard it. The banner is about the coordinator's operational state (archived), not about its message history.

Actually re-reading the design: archived coordinators should show their message history (it's preserved), but with a clear banner indicating the coordinator is archived, and the input area should be disabled. So:

```rust
if is_archived {
    // Show archival banner at top of chat panel
    // Keep existing chat state (don't reset)
    // Disable input area
    self.chat_input_enabled = false;
} else {
    self.chat_input_enabled = true;
    // ... existing chat loading logic
}
```

### Change 3: Visual State Indicators in Tab Bar

The coordinator tab bar should visually distinguish states:

| State | Tab Appearance |
|-------|----------------|
| Active | Normal text, no badge |
| Active + has unread | Bright text + `•` dot indicator |
| Archived | Dimmed text (50% opacity), `archived` badge |
| Active coordinator being viewed | Highlighted background |

**Implementation**: The `CoordinatorTabHit` struct already has `kind: TabBarEntryKind::Coordinator(u32)`. We need to extend it or look up state during rendering:

```rust
// In render loop, for each coordinator tab:
let is_archived = coordinator.tags.iter().any(|t| t == "archived");
let is_active = coordinator.status == Status::InProgress;
let style = match (is_archived, is_active) {
    (true, _) => Style::default().dim(),
    (false, true) if is_selected => selected_style,
    (false, true) => active_style,
    _ => default_style,
};
```

### Change 4: Resume Coordinator Flow

When user selects "Resume" from the Fresh Start choice:

```
┌─ Resume Coordinator ────────────────────────────────────────────────────┐
│                                                                         │
│   Select an archived coordinator to reactivate:                        │
│                                                                         │
│   [0] Coordinator 0 — archived 2h ago — "Design task workflow"         │
│   [1] Coordinator 1 — archived yesterday — "Fix login bug"             │
│                                                                         │
│   [C] Cancel                                                            │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

Implementation:
- Add `ChoiceDialogAction::ResumeArchivedCoordinator`
- Populate options from `graph.tasks()` where `tags.contains("archived")` and `id.starts_with(".coordinator")`
- On selection: call `resume_coordinator(cid)` which sends IPC
- IPC handler removes `archived` tag, sets status to `InProgress`

### Change 5: Dedicated "Fresh Start" Button Label

Change the tab bar `+` button tooltip from "New Coordinator" to clarify intent:

```
[+] New (Fresh Start)
```

Tooltip: "Create a brand new coordinator with a clean slate"

---

## Design: IPC / Backend Changes

### Change 6: Add ResumeCoordinator IPC

```rust
// src/commands/service/ipc.rs

/// Handle ResumeCoordinator IPC request.
/// Removes archived tag and re-activates the coordinator.
fn handle_resume_coordinator(dir: &Path, coordinator_id: u32) -> IpcResponse {
    let graph_path = crate::commands::graph_path(dir);
    let task_id = format!(".coordinator-{}", coordinator_id);
    // ... verify it exists and is archived ...
    // ... set status = InProgress, remove "archived" tag, add log entry ...
}
```

### Change 7: Clarify is_coordinator_slot_available semantics

The current code comment says "Archived and abandoned coordinators exist with their old state and must NOT be resurrected by re-using their ID." The code does this correctly — it treats archived slots as occupied. But the comment should be clearer:

```rust
/// Checks if a coordinator ID is available for a NEW coordinator.
///
/// A slot is available ONLY if:
/// - No task exists with that ID, OR
/// - A task exists but has NO coordinator-loop tag (not a coordinator)
///
/// A slot is NOT available if:
/// - An active coordinator exists (status=InProgress, has coordinator-loop tag)
/// - An archived coordinator exists (must NEVER be resurrected — new ID only)
/// - An abandoned coordinator exists (must NEVER be resurrected — new ID only)
///
/// This ensures that "create new coordinator" ALWAYS gets a fresh ID,
/// never resurrecting an old coordinator's state.
```

---

## Design: Narrative Summary

### Scenario A: User wants a genuinely fresh coordinator

1. User presses `+` in tab bar
2. Choice dialog: "Start fresh" vs "Resume" vs "Cancel"
3. User selects "Start fresh"
4. Name prompt appears (optional)
5. System creates `.coordinator-N` with new ID (next available, never archived)
6. Tab bar shows new coordinator immediately
7. User has completely clean slate

### Scenario B: User wants to resume an archived coordinator

1. User presses `+` in tab bar
2. Choice dialog appears
3. User selects "Resume"
4. List of archived coordinators shown (with archival timestamps)
5. User selects coordinator-1
6. System: removes `archived` tag, sets status to `InProgress`
7. Tab bar now shows coordinator-1 as active
8. User sees preserved chat history (it was never deleted)
9. User can continue from where they left off

### Scenario C: User archives a coordinator, then switches back to it

1. User is on coordinator-1, presses `A` to archive
2. Coordinator-1 gains `archived` tag, status → Done
3. System switches user to next available coordinator
4. Tab bar: coordinator-1 is dimmed with "archived" badge
5. If user clicks coordinator-1 tab:
   - Archival banner shown: "📦 This coordinator was archived. Message history is preserved."
   - Chat history displayed (read-only)
   - Input area disabled
   - User cannot send messages (coordinator is dormant)

### Scenario D: User tries to switch to archived coordinator via Tabs

1. `switch_coordinator(archived_id)` is called
2. System detects `archived` tag on the coordinator task
3. Archival banner displayed in chat panel
4. Chat history is shown (read-only, preserved)
5. Input area disabled
6. Banner includes: "Resume this coordinator" button/keybinding

---

## Implementation Phases

### Phase 1: State Detection (Low Risk)
- [ ] Add `show_archival_banner: bool` and `chat_input_enabled: bool` to `AppState`
- [ ] In `switch_coordinator()`, detect archived coordinators and set these flags
- [ ] In render, show archival banner and disable input when `chat_input_enabled = false`

### Phase 2: Choice Dialog for Fresh/Resume (Medium Risk)
- [ ] Add `ChoiceDialogAction::NewOrResumeCoordinator` variant
- [ ] Change `+` keybinding from `TextPrompt(CreateCoordinator)` to `Choice(NewOrResumeCoordinator)`
- [ ] Add `ChoiceDialogAction::ResumeArchivedCoordinator` variant
- [ ] Implement resume coordinator list with archival timestamp + description

### Phase 3: Resume IPC (Medium Risk)
- [ ] Add `IpcRequest::ResumeCoordinator { coordinator_id }` variant
- [ ] Add `handle_resume_coordinator()` function
- [ ] Wire up in `ipc.rs` handler match
- [ ] Add `resume_coordinator(cid)` method to `AppState`

### Phase 4: Visual Tab Bar State (Low Risk)
- [ ] Extend `CoordinatorTabHit` or compute state during render
- [ ] Apply dimmed style for archived tabs
- [ ] Add archival badge to tab label

### Phase 5: Cleanup (Low Risk)
- [ ] Update `is_coordinator_slot_available` comment for clarity
- [ ] Update tooltip on `+` button
- [ ] Test all four scenarios manually

---

## Files to Modify

| File | Changes |
|------|---------|
| `src/tui/viz_viewer/state.rs` | Add archival detection in `switch_coordinator()`, add `show_archival_banner`, `chat_input_enabled` fields |
| `src/tui/viz_viewer/event.rs` | Change `+` keybinding to show choice dialog first |
| `src/tui/viz_viewer/render.rs` | Show archival banner in chat panel, apply dimmed style to archived tabs |
| `src/tui/viz_viewer/state.rs` (`ChoiceDialogAction`) | Add `NewOrResumeCoordinator` and `ResumeArchivedCoordinator` variants |
| `src/commands/service/ipc.rs` | Add `ResumeCoordinator` IPC variant and handler |
| `src/commands/service/ipc.rs` | Clarify `is_coordinator_slot_available` comment |

---

## Out of Scope

- **Chat history deletion**: Archiving a coordinator does NOT delete its chat history. That data is preserved. The design is about state management, not data deletion.
- **Cross-coordinator message forwarding**: When an archived coordinator receives a message, it stays dormant. No auto-forwarding.
- **Coordinator compaction**: The internal context compaction mechanism is orthogonal to this lifecycle design.
- **Multi-user coordination**: This design assumes single-user TUI session.

---

## Success Criteria

1. **Fresh start is truly fresh**: Creating a new coordinator NEVER inherits state from an archived one. New ID, clean slate.
2. **Archival is terminal and visible**: Archived coordinators are clearly marked (tab bar dimming + badge), their chat is read-only, and they don't bleed through to active sessions.
3. **Resume is explicit**: User must consciously choose "Resume" to reactivate an archived coordinator. It's not an accident.
4. **State isolation**: Switching to an archived coordinator shows it as archived (with preserved history), not as active.
5. **TUI clarity**: At any point, the user can tell which coordinators exist, which are active, and which are archived.
