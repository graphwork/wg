# TUI Multi-Panel Layout Design

## Status: Design (March 2026)

## Overview

This document designs the evolution of `wg tui` from a single-panel viz viewer into a multi-panel control surface supporting graph visualization, task detail, chat with the coordinator agent, task creation/editing, agent monitoring, and quick actions.

## Current Architecture

### File Structure

```
src/tui/
├── mod.rs               # Re-exports viz_viewer
└── viz_viewer/
    ├── mod.rs            # run() entry point, terminal setup/teardown
    ├── state.rs          # VizApp struct (~600 lines), all application state
    ├── render.rs         # draw() + helpers (~900 lines), single-panel rendering
    └── event.rs          # Event loop + key/mouse handlers (~297 lines)
```

### Current State (VizApp)

The `VizApp` struct owns all state in a single flat struct: viz content, viewport scroll, search/filter, task selection, edge tracing, HUD detail, mouse state, live refresh. The render function uses `Layout::Horizontal` conditionally — when HUD is active and terminal width ≥ 100 columns, it renders a side panel; otherwise a bottom panel.

### Key Existing Patterns

- **Conditional side panel**: `render.rs` already splits main area into viz + HUD using `Layout::direction(Horizontal).constraints([Min(1), Length(hud_width)])` when width ≥ `HUD_SIDE_MIN_WIDTH` (100).
- **Modal input**: Search mode (`/`) takes over key handling entirely — `handle_search_input` vs `handle_normal_key`.
- **Lazy loading**: HUD detail is loaded on demand (`load_hud_detail`) and invalidated on selection change.
- **Auto-refresh**: Graph data refreshes every 1500ms by checking file mtime, with no re-render unless data changed.
- **Status bar**: Bottom line shows task counts, token usage, scroll position, search state, live indicator, help hint.

## Panel Architecture

### Layout Mockup — Standard (≥120 col)

```
┌─────────────────────────────────────────────────────────────────────┐
│ Status: 45 tasks (30✓ 5⟳ 8○ 2✗) │ 3 agents │ Service ● │ ?:help  │
├──────────────────────────────────┬──────────────────────────────────┤
│                                  │ [Chat] [Detail] [Agents]        │
│                                  ├──────────────────────────────────┤
│   Graph Visualization            │                                  │
│   (existing viz panel)           │  (active right panel content)    │
│                                  │                                  │
│                                  │                                  │
│                                  │                                  │
│                                  │                                  │
├──────────────────────────────────┴──────────────────────────────────┤
│ a:add  e:edit  d:done  f:fail  r:retry  m:msg  /:search  Tab:panel│
└─────────────────────────────────────────────────────────────────────┘
```

### Layout Mockup — Narrow (<100 col)

```
┌──────────────────────────────────────────┐
│ 45 tasks (30✓ 5⟳ 8○ 2✗) │ Service ●    │
├──────────────────────────────────────────┤
│                                          │
│   Graph Visualization                    │
│                                          │
├──────────────────────────────────────────┤
│  (bottom panel: Detail/Chat/Agents)      │
│                                          │
├──────────────────────────────────────────┤
│ a:add d:done /:search Tab:panel          │
└──────────────────────────────────────────┘
```

### Layout Mockup — Right Panel Collapsed

```
┌─────────────────────────────────────────────────────────────────────┐
│ Status: 45 tasks (30✓ 5⟳ 8○ 2✗) │ 3 agents │ Service ●            │
├─────────────────────────────────────────────────────────────────────┤
│                                                                     │
│   Graph Visualization (full width)                                  │
│                                                                     │
│                                                                     │
├─────────────────────────────────────────────────────────────────────┤
│ a:add  e:edit  d:done  f:fail  /:search  \\:panel                   │
└─────────────────────────────────────────────────────────────────────┘
```

### Layout Structure (Ratatui)

```
Vertical [
  Length(1)     → top status bar
  Min(1)        → middle area (splits further)
  Length(1)     → bottom action hints bar
]

Middle area (when right panel visible, width ≥ 100):
  Horizontal [
    Min(1)           → viz panel (graph)
    Length(right_w)   → right panel (switchable)
  ]

Middle area (when right panel visible, width < 100):
  Vertical [
    Min(1)           → viz panel (graph)
    Length(bottom_h)  → bottom panel (switchable)
  ]

Right panel inner:
  Vertical [
    Length(1)    → tab bar ([Chat] [Detail] [Agents])
    Min(1)      → panel content
  ]
```

## Panel Types

### 1. Graph Visualization Panel (Left)

The existing viz viewer — unchanged. Renders `wg viz` output with ANSI colors, search highlighting, edge tracing, task selection.

**State**: Existing `VizApp` scroll/search/trace state. No changes needed.

#### Note: Default Task Visibility Should Favor Human Work

As the graph gets large, showing dot-prefixed system tasks by default makes it materially harder for both humans and agents to understand what is going on. The default graph view should bias toward the currently relevant human-facing work:

- active and recently-created non-system tasks
- open tasks with meaningful user titles
- in-progress work and its nearby dependencies

Dot-prefixed system tasks should remain available, but as an explicit reveal rather than the default baseline. The TUI should make that discoverable without making the graph noisy. A good default shape would be:

- hide system tasks by default
- show a small reminder or badge that system tasks are hidden
- provide a clear reveal action such as "show system tasks"
- optionally bias the initial view toward recent/open/active tasks when the graph is very large

The product reason is not just visual cleanliness. Large amounts of system-task noise degrade graph legibility and make it harder to validate coordinator behavior, compaction behavior, and the actual state of active work.

### 2. Task Detail Panel (Right Tab: "Detail")

Evolution of the existing HUD. Shows full task information for the selected task.

**Content sections** (scrollable):
- Header: task ID, title, status badge
- Description (full text, word-wrapped)
- Dependencies (with status indicators)
- Dependents (with status indicators)
- Tags, skills, exec mode
- Agent assignment (if any): agent name, role, runtime
- Logs (recent entries, scrollable)
- Artifacts (file list)
- Token usage

**State**: Reuses existing `HudDetail` and `hud_scroll`. Extend `HudDetail.rendered_lines` to include more sections.

### 3. Chat Panel (Right Tab: "Chat")

Conversational interface with the coordinator agent. Requires Phase 2 (coordinator agent) to be functional. Until then, shows a placeholder or sends messages via `wg msg send`.

**Layout**:
```
┌──────────────────────────┐
│ Chat with Coordinator    │
├──────────────────────────┤
│                          │
│ user: plan auth system   │
│                          │
│ coordinator: I'll create │
│   tasks for auth:        │
│   1. Research patterns   │
│   2. Implement JWT...    │
│                          │
│                          │
├──────────────────────────┤
│ > input area_            │
└──────────────────────────┘
```

**Content**:
- Message history: scrollable list of `(role, timestamp, text)` entries
- Input area: single-line text input at bottom (Enter to send)
- Streaming indicator when coordinator is responding

**State**:
```rust
pub struct ChatState {
    /// Message history for display.
    pub messages: Vec<ChatMessage>,
    /// Current input buffer.
    pub input: String,
    /// Scroll offset in message history.
    pub scroll: usize,
    /// Whether coordinator is currently responding.
    pub awaiting_response: bool,
}

pub struct ChatMessage {
    pub role: ChatRole, // User | Coordinator | System
    pub timestamp: String,
    pub text: String,
}
```

**Data flow**:
- Send: `wg chat "message"` (or `wg msg send coordinator "message"` before `wg chat` exists)
- Receive: Poll `.wg/chat/outbox.jsonl` (or `wg msg poll coordinator`) on refresh tick
- Until Phase 2, chat panel can still display messages sent/received via the message queue

### 4. Agent Monitor Panel (Right Tab: "Agents")

Live view of active agents and their status.

**Layout**:
```
┌──────────────────────────┐
│ Active Agents (3)        │
├──────────────────────────┤
│ ● agent-12               │
│   Task: auth-research    │
│   Role: Programmer       │
│   Runtime: 5m 23s        │
│   Tokens: 12.4k in/3.2k │
│                          │
│ ● agent-15               │
│   Task: api-endpoints    │
│   Role: Architect        │
│   Runtime: 2m 10s        │
│                          │
│ ○ agent-8  (idle)        │
│   Last: jwt-impl (done)  │
├──────────────────────────┤
│ Total: 3 active, 1 idle  │
└──────────────────────────┘
```

**State**:
```rust
pub struct AgentMonitorState {
    /// Agent entries loaded from the agent registry.
    pub agents: Vec<AgentEntry>,
    /// Scroll offset.
    pub scroll: usize,
}

pub struct AgentEntry {
    pub agent_id: String,
    pub task_id: Option<String>,
    pub role: String,
    pub status: AgentStatus, // Running | Idle | Failed
    pub runtime: Duration,
    pub tokens: Option<TokenUsage>,
}
```

**Data flow**: Read from `AgentRegistry` (already loaded by `VizApp::load_stats`) on each refresh tick.

## State Architecture

### Panel Focus

```rust
/// Which panel currently has keyboard focus.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FocusedPanel {
    Graph,
    RightPanel,
}

/// Which tab is active in the right panel.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RightPanelTab {
    Detail,
    Chat,
    Agents,
}
```

### New State Fields on VizApp

```rust
// ── Panel layout ──
/// Whether the right panel is visible (toggle with `\`).
pub right_panel_visible: bool,
/// Which panel has keyboard focus.
pub focused_panel: FocusedPanel,
/// Active tab in the right panel.
pub right_panel_tab: RightPanelTab,
/// Right panel width as percentage of terminal width (default 35).
pub right_panel_percent: u16,

// ── Input mode ──
/// Current input mode (replaces the boolean `search_active`).
pub input_mode: InputMode,

// ── Chat state ──
pub chat: ChatState,

// ── Agent monitor state ──
pub agent_monitor: AgentMonitorState,
```

### Input Mode

```rust
/// Input modes — at most one is active at a time.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Normal navigation mode. Keys go to the focused panel.
    Normal,
    /// Search mode (/ key). Keys go to search input.
    Search,
    /// Chat input mode. Keys go to chat text input.
    ChatInput,
    /// Task creation form. Keys go to form fields.
    TaskForm,
    /// Confirmation dialog (e.g., "Mark task done? y/n").
    Confirm,
}
```

## Focus Management

### Rules

1. Exactly one panel has focus at a time (indicated by border color: yellow = focused, dark gray = unfocused).
2. `Tab` switches focus between Graph and RightPanel (when right panel is visible). This replaces the current Tab behavior (trace toggle). Trace toggle moves to `t` (currently token toggle — token toggle moves to `T`).
3. When focus is on RightPanel, `1`/`2`/`3` switch tabs (Detail/Chat/Agents), or left/right arrow keys cycle tabs.
4. When focus is on Graph, all existing navigation keys work as before.
5. When focus is on RightPanel, Up/Down/PgUp/PgDn scroll the active panel's content.
6. `\` (backslash) toggles right panel visibility. When collapsed, all focus is on Graph.
7. Entering an input mode (search, chat input, task form) takes exclusive focus until exited with Enter/Esc.

### Key Binding Table

#### Global Keys (work in any mode except input modes)

| Key | Action | Notes |
|-----|--------|-------|
| `Tab` | Switch focus: Graph ↔ Right Panel | Replaces trace toggle |
| `\` | Toggle right panel visibility | Collapse/expand |
| `?` | Show help overlay | Existing |
| `q` | Quit | Existing |
| `Ctrl-c` | Force quit | Existing |
| `r` | Force refresh | Existing |
| `/` | Enter search mode | Existing |
| `Esc` | Clear search / exit mode / quit | Context-dependent |

#### Graph Panel Keys (focus on Graph, Normal mode)

| Key | Action | Notes |
|-----|--------|-------|
| `↑`/`↓` | Select prev/next task | Edge tracing always on when task selected |
| `j`/`k` | Scroll down/up | Vim-style |
| `h`/`l` | Scroll left/right | Vim-style |
| `Ctrl-d`/`Ctrl-u` | Page down/up | Vim-style |
| `g`/`G` | Jump to top/bottom | Vim-style |
| `n`/`N` | Next/prev search match | Existing |
| `m` | Toggle mouse | Existing |
| `L` | Cycle layout mode | Existing |
| `t` | Toggle trace visibility | Moved from Tab |
| `T` | Toggle view/total tokens | Moved from t |

#### Quick Action Keys (focus on Graph, task selected)

| Key | Action | Notes |
|-----|--------|-------|
| `a` | Open task creation form | New |
| `e` | Open task edit form | New |
| `d` | Mark selected task done | Confirm dialog |
| `f` | Mark selected task failed | Confirm dialog + reason |
| `x` | Retry selected task | Confirm dialog |
| `c` | Open chat input (right panel switches to Chat) | New |

#### Right Panel Keys (focus on Right Panel)

| Key | Action | Notes |
|-----|--------|-------|
| `1` | Switch to Detail tab | |
| `2` | Switch to Chat tab | |
| `3` | Switch to Agents tab | |
| `←`/`→` | Cycle tabs | |
| `↑`/`↓` | Scroll panel content | |
| `PgUp`/`PgDn` | Fast scroll | |
| `Enter` | Enter chat input (Chat tab only) | |

#### Search Mode Keys (unchanged from current)

| Key | Action |
|-----|--------|
| Characters | Type search query |
| `Backspace` | Delete character |
| `Ctrl-u` | Clear input |
| `Enter` | Accept search and jump |
| `Esc` | Cancel search |
| `Tab`/`Shift-Tab` | Next/prev match |

#### Chat Input Mode Keys

| Key | Action |
|-----|--------|
| Characters | Type message |
| `Backspace` | Delete character |
| `Ctrl-u` | Clear input |
| `Enter` | Send message |
| `Esc` | Exit chat input mode |
| `↑`/`↓` | Scroll message history |

#### Confirm Dialog Keys

| Key | Action |
|-----|--------|
| `y`/`Enter` | Confirm action |
| `n`/`Esc` | Cancel |

## Component Hierarchy

### Proposed File Structure

```
src/tui/
├── mod.rs                    # run() entry point, terminal setup/teardown
├── app.rs                    # TuiApp: top-level state + dispatch
├── panels/
│   ├── mod.rs
│   ├── graph.rs              # Graph viz panel (extracted from current render.rs)
│   ├── detail.rs             # Task detail panel (evolved from HUD)
│   ├── chat.rs               # Chat panel
│   ├── agents.rs             # Agent monitor panel
│   └── status_bar.rs         # Top status bar + bottom action hints
├── widgets/
│   ├── mod.rs
│   ├── tab_bar.rs            # Tab switcher widget for right panel
│   ├── confirm_dialog.rs     # Confirmation overlay (y/n)
│   ├── task_form.rs          # Task creation/edit form overlay
│   └── text_input.rs         # Reusable text input widget (chat, search, forms)
├── event.rs                  # Top-level event dispatch
└── viz_viewer/               # Preserved for backward compatibility during transition
    ├── mod.rs
    ├── state.rs
    ├── render.rs
    └── event.rs
```

### Migration Strategy

The refactoring should be incremental — the existing `viz_viewer` module continues to work throughout. The new panel system wraps the existing viz viewer rather than rewriting it.

**Phase 3a (sh-tui-panels-and-actions)**:
1. Create `app.rs` with `TuiApp` that wraps `VizApp` + panel state
2. Create `panels/` with graph panel delegating to `VizApp` render/event
3. Add right panel framework (tab bar + empty panels)
4. Implement focus switching and panel collapse
5. Add quick action keys (d/f/x → confirm dialog → `wg done/fail/retry`)
6. Add task creation form (a → form → `wg add`)

**Phase 3b (sh-tui-chat-panel)**:
1. Implement `panels/chat.rs` with message display + input
2. Wire up to `wg chat` or message queue
3. Add streaming response display

### Component Contracts

Each panel implements a common interface:

```rust
pub trait Panel {
    /// Handle a key event. Returns true if consumed.
    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> bool;

    /// Handle a mouse event within the panel's area.
    fn handle_mouse(&mut self, kind: MouseEventKind, row: u16, col: u16);

    /// Render the panel into the given area.
    fn draw(&self, frame: &mut Frame, area: Rect, focused: bool);

    /// Called on each refresh tick to update data.
    fn refresh(&mut self);
}
```

> **Implementation note**: This trait is a conceptual guide for the panel interface contract. Whether it's implemented as an actual Rust trait or as a set of free functions per panel module is an implementation decision for the downstream tasks. Ratatui's rendering model (taking `&mut Frame`) and the need for shared state access (e.g., selected task ID flowing from graph to detail panel) may make free functions with explicit state parameters cleaner than trait objects. The key contract is: each panel handles its own keys, renders into a `Rect`, and refreshes independently.

## Panel Sizing

### Configurable Split Ratios

```rust
/// Panel sizing configuration.
pub struct PanelSizing {
    /// Right panel width as percentage of terminal width (5..=80, default 35).
    pub right_panel_percent: u16,
    /// Bottom panel height as percentage of terminal height (5..=80, default 40).
    pub bottom_panel_percent: u16,
    /// Minimum terminal width for side-by-side layout.
    pub side_min_width: u16, // default 100
}
```

### Resize Keys

| Key | Action |
|-----|--------|
| `Ctrl-Left` | Shrink right panel by 5% |
| `Ctrl-Right` | Grow right panel by 5% |

### Responsive Behavior

- **Width ≥ 100**: Side-by-side layout (graph left, right panel right)
- **Width < 100**: Stacked layout (graph top, panel bottom)
- **Right panel collapsed**: Graph takes full width, bottom bar still visible
- **Full-screen mode** (`F` key): Active panel takes entire screen. Press `F` or `Esc` to return.

## Visual Indicators

### Focus

- **Focused panel**: Border color yellow, border style `Borders::ALL`
- **Unfocused panel**: Border color dark gray, border style `Borders::ALL`
- **No border on graph panel** when it's the only panel (matches current behavior)

### Tab Bar

```
 ▸ Chat │ Detail │ Agents
```

Active tab: bold + yellow. Inactive: dim. Using `▸` marker on active tab.

### Status Bar (Top)

Evolves the current bottom status bar to the top. Contains:
- Task counts with status-colored badges
- Active agent count
- Service status (● running / ○ stopped)
- Token usage (toggled view/total)
- Help hint

### Action Hints Bar (Bottom)

Context-sensitive hints showing available actions:
- Graph focus: `a:add  e:edit  d:done  f:fail  x:retry  /:search  Tab:panel  \:collapse`
- Chat focus: `Enter:input  Tab:panel  1-3:tab`
- Search mode: `Tab:next  S-Tab:prev  Enter:go  Esc:cancel`
- Chat input: `Enter:send  Esc:cancel`

## Data Flow

### Refresh Cycle

```
Every 1500ms (existing interval):
  1. Check graph.jsonl mtime (existing)
  2. If changed:
     a. Reload viz data (existing)
     b. Reload task counts + token usage (existing)
     c. Reload agent registry → update AgentMonitorState
     d. If detail tab active + task selected → reload HudDetail
  3. If chat tab active:
     a. Poll chat outbox for new messages
     b. Append new messages to ChatState.messages
```

### Cross-Panel Communication

Panels share state through `TuiApp` fields, not through message passing:
- Graph panel writes `selected_task_idx` → Detail panel reads it
- Quick action keys (d/f) on graph → executes `wg done/fail` → triggers graph refresh
- Chat send → writes message → next refresh picks up response
- Task creation form → executes `wg add` → triggers graph refresh

### Command Execution

Quick actions and forms execute `wg` CLI commands in a background thread to avoid blocking the render loop:

```rust
/// Queue of commands to execute in the background.
pub struct CommandQueue {
    pending: Vec<PendingCommand>,
    results: Vec<CommandResult>,
}

pub struct PendingCommand {
    pub command: String,  // e.g., "wg done my-task"
    pub on_success: CommandEffect, // e.g., ForceRefresh
}
```

The event loop drains `CommandQueue.results` each tick and applies effects (refresh, show notification, etc.).

## Overlay Widgets

### Confirmation Dialog

```
┌─────────────────────────────┐
│ Mark 'auth-research' done?  │
│                             │
│         [y] Yes  [n] No     │
└─────────────────────────────┘
```

Centered overlay. Blocks all other input until dismissed.

### Task Creation Form

```
┌─ Create Task ──────────────────────────┐
│                                        │
│ Title:  ________________________________│
│                                        │
│ After:  auth-research, jwt-design      │
│         (fuzzy search: type to filter) │
│                                        │
│ Tags:   self-hosting, phase-3          │
│                                        │
│ Exec:   [full] light  bare  shell      │
│                                        │
│        [Enter: create]  [Esc: cancel]  │
└────────────────────────────────────────┘
```

Centered overlay. Tab to move between fields. Fuzzy completion for dependency task IDs.

### Task Edit Form

Similar to creation but pre-populated with existing task data. Only editable fields shown (description, deps, tags).

## Migration Path from Current TUI

### Breaking Changes

| Current | New | Reason |
|---------|-----|--------|
| `Tab` toggles edge trace | `t` toggles edge trace | `Tab` needed for panel focus switching |
| `t` toggles token display | `T` toggles token display | `t` repurposed for trace toggle |
| Status bar at bottom | Status bar at top | Bottom reserved for action hints |
| No panel concept | Multi-panel with focus | Core architectural change |

### Backward Compatibility

- Graph-only mode (right panel collapsed) behaves almost identically to current TUI
- All vim-style navigation keys unchanged
- Search behavior unchanged
- Mouse behavior unchanged within the graph panel
- Edge tracing still works, just toggled with `t` instead of `Tab`

### Tab Key Migration

The current `Tab` key serves double duty: single press toggles trace, double-tap recenters. This is repurposed:

- `Tab` → switch panel focus (single press)
- `t` → toggle trace (single press, was `Tab`)
- `T` → toggle token display (single press, was `t`)
- Double-tap recentering → removed (use `Tab Tab` to focus graph + then `Ctrl-l` to recenter, or just use the selection-follows-scroll behavior)

## Open Questions for Implementation

1. **Should the right panel border overlap with the graph area?** Current HUD uses `Block::default().borders(Borders::ALL)` which consumes space. The graph panel currently has no border. Using shared borders would save 1 column.

2. **Should chat messages persist across TUI sessions?** If chat reads from `.wg/chat/outbox.jsonl`, messages persist naturally. But the scroll position and "last read" state would need saving.

3. **Should task form support multiline description editing?** A full text editor is complex. Initial implementation could use single-line title only, with description added via `wg edit` after creation. Or a simple multiline textarea with Enter for newlines and Ctrl-Enter to submit.

4. **How should streaming responses render in the chat panel?** Options: (a) show "..." until complete, (b) render partial text and update as it streams. Option (b) is better UX but requires polling the response file during render.

## Dependencies

- **This design (sh-tui-layout-design)**: No code dependencies, standalone design doc.
- **sh-tui-panels-and-actions**: Implements the panel framework, task form, quick actions. Depends on this design.
- **sh-tui-chat-panel**: Implements the chat panel. Depends on this design + Phase 2 coordinator agent (for `wg chat`). Can start with message queue fallback.
