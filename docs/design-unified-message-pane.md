# Design: Unified Message/Chat Window for TUI

**Task:** `design-unified-message`
**Status:** Proposal — awaiting review
**Date:** 2026-03-05

---

## 1. Current State Analysis

### 1.1 Coordinator Chat (Tab 0: "Chat")

**State:** `ChatState` in `src/tui/viz_viewer/state.rs:641`
**Rendering:** `draw_chat_tab()` in `src/tui/viz_viewer/render.rs:1463`
**Input handling:** `handle_chat_input()` in `src/tui/viz_viewer/event.rs:480`
**Input mode:** `InputMode::ChatInput`

| Aspect | Details |
|--------|---------|
| Data model | `ChatMessage { role: ChatRole, text: String, full_text: Option<String>, attachments: Vec<String> }` |
| Storage (IPC) | `src/chat.rs` — JSONL files in `.workgraph/chat/` (`inbox.jsonl` for user→coordinator, `outbox.jsonl` for coordinator→user) |
| Storage (display) | In-memory `Vec<ChatMessage>`, persisted to `.workgraph/chat-history.json` |
| Polling | `poll_chat_messages()` reads `outbox.jsonl` since cursor; called on refresh tick |
| Sending | `send_chat_message()` → `wg chat <text>` background command |
| Scroll model | Lines-from-bottom (0 = fully scrolled down) |
| Features | Markdown rendering, tool-call box formatting, attachments, clipboard image paste, scrollbar, warm-tinted user message background, "awaiting response" spinner |
| Editor | `edtui` `EditorState` with Emacs keybindings |

### 1.2 Per-Task Messages (Tab 3: "Msg")

**State:** `MessagesPanelState` in `src/tui/viz_viewer/state.rs:956`
**Rendering:** `draw_messages_tab()` in `src/tui/viz_viewer/render.rs:2359`
**Input handling:** `handle_message_input()` in `src/tui/viz_viewer/event.rs:536`
**Input mode:** `InputMode::MessageInput`

| Aspect | Details |
|--------|---------|
| Data model | `MessageEntry { sender, display_label, body, timestamp, is_urgent, direction: MessageDirection, delivery_status }` |
| Storage | `src/messages.rs` — JSONL files in `.workgraph/messages/{task-id}.jsonl` with read cursors in `.cursors/` |
| Loading | `load_messages_panel()` calls `workgraph::messages::list_messages()` — full reload per task |
| Sending | `wg msg send <task-id> <text> --from tui` background command |
| Scroll model | Lines-from-top (0 = fully scrolled to top) |
| Features | Incoming/outgoing direction arrows, sender color palette, delivery status icons, urgency markers, unanswered tracking, summary header, markdown body rendering |
| Editor | Same `edtui` `EditorState` with Emacs keybindings |

### 1.3 Task Logs (Tab 2: "Log")

**State:** `LogPaneState` in `src/tui/viz_viewer/state.rs:860`
**Rendering:** `draw_log_tab()` in `src/tui/viz_viewer/render.rs:2158`
**Loading:** `load_log_pane()` in `src/tui/viz_viewer/state.rs:3392`

| Aspect | Details |
|--------|---------|
| Data model | `LogEntry { timestamp, actor: Option<String>, message }` (from `src/graph.rs:77`) |
| Storage | Embedded in task struct within `graph.jsonl` (not a separate file) |
| Loading | Reads from `graph.tasks().find(id).log` — full graph load per task change |
| Scroll model | Lines-from-top with auto-tail (new content scrolls to bottom) |
| Features | Relative timestamps, JSON debug mode toggle, read-only (no input area) |
| Editor | None — read-only pane |

### 1.4 Coordinator Log (Tab 7: "Coord")

**State:** `CoordLogState` in `src/tui/viz_viewer/state.rs:891`
**Rendering:** `draw_coord_log_tab()` in `src/tui/viz_viewer/render.rs:2254`

Read-only daemon activity log. Out of scope for unification (it's operational logging, not messaging).

---

## 2. Commonality and Differences

### 2.1 Shared Patterns

All three messaging surfaces share:

1. **Scrollable message list** — a viewport over rendered `Vec<Line>` with scroll offset
2. **Per-frame scroll state** — `total_wrapped_lines`, `viewport_height`, scrollbar rendering
3. **Word-wrapping** — content is word-wrapped to viewport width
4. **Relative timestamps** — all use `format_relative_time()` for display
5. **Task-ID scoping** — Messages and Logs are scoped to `selected_task_id()`; Chat is scoped to the coordinator (implicit singleton)
6. **Staleness detection** — `task_id: Option<String>` field tracks what's loaded, skips reload if unchanged

Chat and Messages additionally share:

7. **Text input area** — `edtui` `EditorState` with Emacs keybindings, `"> "` prompt prefix
8. **Input/display split layout** — message area on top, input at bottom with separator
9. **Background command execution** — sends via `exec_command()` with `CommandEffect`
10. **Markdown body rendering** — both use `markdown_to_lines()` + `wrap_line_spans()`

### 2.2 Key Differences

| Dimension | Chat | Messages | Log |
|-----------|------|----------|-----|
| **Scope** | Global (coordinator) | Per-task | Per-task |
| **Direction model** | Role-based (User/Coordinator/System) | Direction-based (Incoming/Outgoing) | Actor-tagged (optional) |
| **Scroll origin** | Bottom (0 = newest visible) | Top (0 = oldest visible) | Top with auto-tail |
| **Storage backend** | `chat.rs` (inbox/outbox JSONL pair) | `messages.rs` (single JSONL per task) | `graph.jsonl` (embedded in task) |
| **Polling** | Cursor-based incremental (`outbox_cursor`) | Full reload on invalidate | Full reload from graph |
| **Input** | Yes (send to coordinator) | Yes (send to task) | No (read-only) |
| **Rich formatting** | Tool-call boxes, user bg tint | Direction arrows, sender palette, delivery icons | Plain text |
| **Attachments** | Yes (file + clipboard image) | No | No |
| **State tracking** | `awaiting_response` + `last_request_id` | `summary` (incoming/outgoing/unanswered) | `auto_tail` + `json_mode` |

### 2.3 Assessment: Refactor vs. Fundamental Incompatibility?

**This is purely a refactor.** The differences are all in the *data source* and *presentation details*, not in the fundamental widget structure. Both Chat and Messages already have the same layout: scrollable message list + input area + scrollbar. The rendering logic is parallel code that could be parameterized.

The Log pane is the outlier — it's read-only and its data lives in the graph, not a message queue. Including it in the unified component is possible (as a read-only `MessageSource`) but lower priority. The high-value unification is between Chat and Messages.

---

## 3. Proposed Architecture

### 3.1 `MessageSource` Enum

```rust
/// Where messages come from and how to interact with them.
pub enum MessageSource {
    /// Coordinator chat (global, inbox/outbox model).
    Coordinator,
    /// Per-task message queue (scoped to a task ID).
    Task(String),
}
```

This replaces the current split between `ChatState` and `MessagesPanelState`. The source determines:
- How messages are loaded (chat outbox polling vs. messages list)
- How messages are sent (chat inbox append vs. msg send)
- The display style (role-based vs. direction-based)

### 3.2 `UnifiedMessage` — Common Data Model

```rust
/// A message in the unified pane, regardless of source.
pub struct UnifiedMessage {
    /// Display label for the sender (e.g., "you", "coordinator", "agent", sender ID).
    pub sender_label: String,
    /// Alignment/role: left (incoming/other) or right (outgoing/self).
    pub alignment: MessageAlignment,
    /// Primary text content.
    pub body: String,
    /// Optional full/expanded text (e.g., coordinator tool calls).
    pub expanded_body: Option<String>,
    /// Relative timestamp for display.
    pub timestamp: String,
    /// Optional attachment filenames.
    pub attachments: Vec<String>,
    /// Optional delivery status icon.
    pub delivery_status: Option<DeliveryStatusIcon>,
    /// Whether this message is urgent.
    pub is_urgent: bool,
    /// Whether this message is unanswered.
    pub is_unanswered: bool,
}

pub enum MessageAlignment {
    /// Left-aligned (incoming: coordinator responses, external senders).
    Left,
    /// Right-aligned (outgoing: user messages, agent replies).
    Right,
}
```

Each `MessageSource` produces a `Vec<UnifiedMessage>` via an adapter:
- `Coordinator` → maps `ChatMessage` (role-based) to `UnifiedMessage` (User→Right, Coordinator→Left, System→Left)
- `Task(id)` → maps `MessageEntry` (direction-based) to `UnifiedMessage` (Outgoing→Right, Incoming→Left)

### 3.3 `MessagePane` — Unified Widget

```rust
pub struct MessagePaneState {
    /// Current message source.
    pub source: MessageSource,
    /// Rendered messages (common format).
    pub messages: Vec<UnifiedMessage>,
    /// Editor state for input (shared between both modes).
    pub editor: EditorState,
    /// Scroll offset.
    pub scroll: usize,
    /// Scroll direction convention (configurable per source).
    pub scroll_origin: ScrollOrigin,
    /// Total rendered lines (for scrollbar).
    pub total_rendered_lines: usize,
    /// Viewport height (for scrollbar).
    pub viewport_height: usize,
    /// Source-specific state.
    pub source_state: SourceState,
}

pub enum ScrollOrigin {
    /// 0 = bottom (used by Chat).
    Bottom,
    /// 0 = top (used by Messages).
    Top,
}

pub enum SourceState {
    Coordinator {
        awaiting_response: bool,
        last_request_id: Option<String>,
        outbox_cursor: u64,
        coordinator_active: bool,
        pending_attachments: Vec<PendingAttachment>,
    },
    Task {
        task_id: Option<String>,
        summary: MessageSummary,
    },
}
```

### 3.4 Unified Rendering

A single `draw_message_pane()` function replaces both `draw_chat_tab()` and `draw_messages_tab()`:

```
draw_message_pane(frame, pane_state, area)
├── Layout: message_area + input_area
├── Empty state (parameterized by source)
├── Message rendering loop:
│   ├── Header line (sender label, timestamp, status icons)
│   ├── Body (markdown_to_lines + wrap)
│   ├── Attachments (if any)
│   └── Tool-call boxes (if Coordinator source + expanded_body)
├── Streaming indicator (if Coordinator + awaiting)
├── Scroll + scrollbar
└── draw_message_input(frame, editor, area)
```

The source-specific rendering differences become small branches within the loop:
- **Sender prefix:** `"> "` for Right/User vs. `"↯ "` for Left/Coordinator vs. `"← "/"→ "` for Task direction
- **Background tint:** Only for Right-aligned messages in Coordinator source
- **Tool boxes:** Only for Coordinator-sourced Left messages with `expanded_body`
- **Summary header:** Only for Task source

### 3.5 Unified Input Handling

A single `handle_message_pane_input()` replaces `handle_chat_input()` and `handle_message_input()`. The submit action dispatches based on `source`:

```rust
fn submit_message(app: &mut VizApp, pane: &mut MessagePaneState) {
    let text = editor_text(&pane.editor);
    editor_clear(&mut pane.editor);
    match &pane.source {
        MessageSource::Coordinator => app.send_chat_message(text),
        MessageSource::Task(task_id) => {
            app.exec_command(
                vec!["msg", "send", task_id, &text, "--from", "tui"],
                CommandEffect::Notify(format!("Message sent to '{}'", task_id)),
            );
        }
    }
}
```

### 3.6 Context Switching

The TUI already switches tabs. With the unified pane, switching between Chat and Messages becomes switching the `MessageSource`:

- **Tab 0 (Chat):** `MessageSource::Coordinator` — always available
- **Tab 3 (Msg):** `MessageSource::Task(selected_task_id)` — updates when task selection changes

Optionally, a single `InputMode::MessagePaneInput` replaces both `InputMode::ChatInput` and `InputMode::MessageInput`.

---

## 4. Migration Path

### Phase 1: Extract Common Types (Low Risk)

1. Create `UnifiedMessage` and `MessageAlignment` in `state.rs`
2. Add conversion functions: `ChatMessage → UnifiedMessage` and `MessageEntry → UnifiedMessage`
3. No UI changes yet — just the data model

### Phase 2: Unify Rendering (Medium Risk)

1. Create `draw_message_pane()` that renders `Vec<UnifiedMessage>` with configurable style
2. Extract common rendering helpers: `draw_message_header()`, `draw_message_body()`, `draw_message_input_area()`
3. Rewrite `draw_chat_tab()` and `draw_messages_tab()` as thin wrappers that convert data and call `draw_message_pane()`
4. This is the largest phase — the two render functions are ~300 and ~400 lines respectively

### Phase 3: Unify State (Medium Risk)

1. Replace `ChatState` + `MessagesPanelState` with `MessagePaneState`
2. Keep source-specific state in `SourceState` enum
3. Merge `InputMode::ChatInput` and `InputMode::MessageInput` into one
4. Update all event handling to route through unified handler

### Phase 4: Unify Input Handling (Low Risk)

1. Create `handle_message_pane_input()` that dispatches based on source
2. Remove `handle_chat_input()` and `handle_message_input()`
3. The editor keybinding logic is already identical — just the submit action differs

### Phase 5 (Optional): Log Pane as Read-Only Source

1. Add `MessageSource::TaskLog(String)` variant
2. Convert `LogEntry` → `UnifiedMessage` (all Left-aligned, no input)
3. `draw_message_pane()` hides the input area when source is read-only
4. This would let us remove `draw_log_tab()` entirely

---

## 5. Scope and Complexity Estimate

| Phase | Files Changed | Lines Changed (est.) | Risk |
|-------|--------------|---------------------|------|
| 1: Common types | `state.rs` | ~60 new | Low |
| 2: Rendering | `render.rs`, `state.rs` | ~400 refactored | Medium — largest visual change |
| 3: State unification | `state.rs`, `event.rs` | ~200 refactored | Medium — touches scroll, polling |
| 4: Input handling | `event.rs` | ~100 refactored | Low |
| 5: Log pane (optional) | `render.rs`, `state.rs` | ~100 refactored | Low |

**Total: ~4 tasks if parallelized (Phase 1→[2,3,4]→5), or a single focused task for Phases 1-4.**

The main risk is Phase 2 (rendering unification) because the two render functions have accumulated source-specific visual details (tool boxes, sender color cycling, delivery icons). These need to be preserved while consolidating the layout logic.

---

## 6. Open Questions for the User

1. **Should the Log pane be included in Phase 1 or deferred?** It's the cleanest conceptually (read-only message source) but lowest value since it's already functional and simple.

2. **Single tab or two tabs?** Currently Chat is Tab 0 and Messages is Tab 3. Options:
   - **Keep both tabs** but backed by the same component (minimal UX change)
   - **Merge into one tab** that shows coordinator chat by default and switches to task messages when a task is focused
   - **Keep both but unify input mode** — pressing Enter/c in either tab enters the same input mode

3. **Scroll convention:** Chat scrolls from bottom (newest visible), Messages scrolls from top. Should we standardize on one? Bottom-origin (like Chat) is more natural for real-time conversation. This would change Messages behavior.

4. **Attachment support for task messages?** Chat has file/image attachments. Should this be extended to task messages in the unified model? The storage layer (`messages.rs`) doesn't currently support attachments.

5. **Incremental loading for Messages?** Currently Messages does a full reload per task change. Chat uses incremental cursor-based polling. Should Messages adopt the same pattern for consistency?

---

## 7. Recommendation

**Proceed with Phases 1-4 as a single implementation effort**, keeping both tabs but sharing the unified component. This gives:

- **~60% code reduction** in render.rs message-related code (two 300-400 line functions → one ~350 line function + two thin wrappers)
- **Single input mode** instead of two parallel ones
- **Consistent behavior** — same scroll, same keybindings, same visual language
- **Future-proof** — adding new message sources (e.g., inter-agent messaging panel) becomes trivial

Defer Phase 5 (Log pane) until after the core unification is stable.
