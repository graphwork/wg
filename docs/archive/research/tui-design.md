# TUI Design for workgraph

This document outlines the design for a terminal user interface (TUI) for workgraph using ratatui.

## Table of Contents

1. [Overview](#1-overview)
2. [Core Views](#2-core-views)
3. [Key Bindings](#3-key-bindings)
4. [Layout Options](#4-layout-options)
5. [Status Bar](#5-status-bar)
6. [Technical Approach](#6-technical-approach)
7. [ASCII Mockups](#7-ascii-mockups)
8. [MVP Scope](#8-mvp-scope)
9. [Future Extensions](#9-future-extensions)

---

## 1. Overview

### Goals

- Provide an interactive, real-time view of the workgraph
- Enable fast task management without leaving the terminal
- Support both human operators and monitoring of agent activity
- Maintain the principle of simplicity while adding power-user features

### Non-Goals (for MVP)

- Full graph editing (complex dependency modifications)
- Multi-user collaboration features
- Remote workgraph viewing

---

## 2. Core Views

### 2.1 Task List View

The primary view showing all tasks with filtering and sorting capabilities.

**Displayed Information:**
- Status indicator: `[ ]` open, `[~]` in-progress, `[x]` done
- Task ID (truncated if needed)
- Estimated hours
- Assignee (or `@unassigned`)
- Derived state: `ready`, `blocked`, `in-prog`, `done`
- Tags (if space permits)

**Filtering:**
- By status: `open`, `in-progress`, `done`, `blocked`, `ready`
- By assignee: `@alice`, `@bob`, `@unassigned`
- By tag: `#frontend`, `#backend`, `#urgent`
- Combined filters: `status:ready tag:urgent`

**Sorting:**
- By created date (default)
- By estimated hours
- By number of dependents (impact)
- By age (oldest first)

### 2.2 Task Detail View

Shows complete information for a selected task.

**Displayed Information:**
- Full task title
- Task ID
- Status with timestamps (created, started, completed)
- Assignee
- Estimate (hours/cost)
- Tags
- Blockers list with their status
- Dependents list (what this task blocks)
- Resource requirements

### 2.3 Ready Queue View

A focused view of tasks that can be worked on immediately.

**Displayed Information:**
- Only tasks where all blockers are `done` and `not_before` has passed
- Sorted by impact (number of tasks this unblocks)
- Shows estimated hours for capacity planning
- Quick-claim functionality

### 2.4 Graph View

ASCII visualization of task dependencies.

**Display Modes:**
- Tree view: show dependency chains from selected task
- Neighborhood view: show immediate blockers and dependents
- Critical path highlight: emphasize the longest chain

**Example Tree:**
```
api-impl (in-progress)
├── api-design [done]
└── database-schema [done]
    └── requirements-doc [done]

Blocked by api-impl:
├── api-tests (ready when api-impl done)
├── frontend-integration (blocked)
│   └── also needs: ui-design [in-progress]
└── documentation (blocked)
```

---

## 3. Key Bindings

### Navigation

| Key | Action |
|-----|--------|
| `j` / `Down` | Move cursor down |
| `k` / `Up` | Move cursor up |
| `g` / `Home` | Go to first item |
| `G` / `End` | Go to last item |
| `Ctrl+d` | Page down |
| `Ctrl+u` | Page up |

### Actions

| Key | Action |
|-----|--------|
| `Enter` | View task details / expand |
| `c` | Claim selected task |
| `d` | Mark selected task as done |
| `u` | Unclaim selected task |
| `a` | Add new task (opens input modal) |
| `e` | Edit task (future) |
| `r` | Refresh from disk |

### View Switching

| Key | Action |
|-----|--------|
| `1` | Task list view |
| `2` | Ready queue view |
| `3` | Graph view |
| `Tab` | Cycle through views |
| `Esc` | Close modal / go back |

### Filtering & Search

| Key | Action |
|-----|--------|
| `/` | Open search/filter input |
| `f` | Quick filter menu |
| `s` | Cycle sort order |
| `Ctrl+l` | Clear filter |

### Other

| Key | Action |
|-----|--------|
| `?` | Show help overlay |
| `q` | Quit |
| `Ctrl+c` | Force quit |

---

## 4. Layout Options

### 4.1 Single Pane (Default)

Full-screen list view, detail shown on Enter as overlay.

```
┌─ Tasks ─────────────────────────────────────────────────────────┐
│ [ ] api-design              8h  @unassigned         ready       │
│ [~] database-work           4h  @alice              in-prog     │
│ [x] setup-ci                2h  @bob                done        │
│ [ ] frontend-components    12h  @unassigned         blocked     │
│ [ ] api-tests               6h  @unassigned         blocked     │
│ ...                                                             │
├─────────────────────────────────────────────────────────────────┤
│ Filter: all | Open: 15 | In-Progress: 3 | Done: 8 | @alice      │
└─────────────────────────────────────────────────────────────────┘
```

### 4.2 Split Pane (Detail)

List on left, detail on right. Toggled with `v`.

```
┌─ Tasks ──────────────────────┬─ Detail ─────────────────────────┐
│ [ ] api-design         ready │ Title: Database Schema Design    │
│ [~] database-work    in-prog │ ID: database-work                │
│ [x] setup-ci           done  │ Status: in-progress              │
│ [ ] frontend-comp    blocked │ Assigned: @alice                 │
│ [ ] api-tests        blocked │ Estimate: 4h                     │
│                              │ Started: 2024-01-15 10:30        │
│                              │                                  │
│                              │ Blocked By:                      │
│                              │   [x] setup-ci                   │
│                              │                                  │
│                              │ Blocks:                          │
│                              │   [ ] api-impl                   │
│                              │   [ ] data-migration             │
├──────────────────────────────┴──────────────────────────────────┤
│ Filter: all | Open: 15 | In-Progress: 3 | Done: 8 | @alice      │
└─────────────────────────────────────────────────────────────────┘
```

### 4.3 Tab-Based

Different views as tabs, switch with number keys or Tab.

```
┌─[1:Tasks]─[2:Ready]─[3:Graph]─[4:Actors]────────────────────────┐
│                                                                 │
│ Ready Queue (5 tasks)                                           │
│                                                                 │
│  1. api-design           8h  Unblocks 3 tasks                   │
│  2. write-tests          2h  Unblocks 1 task                    │
│  3. update-docs          1h  Leaf task                          │
│  4. refactor-auth        4h  Unblocks 2 tasks                   │
│  5. fix-bug-123          1h  Leaf task                          │
│                                                                 │
│ Total ready work: 16h                                           │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│ Ready: 5 | Total Remaining: 45h | Velocity: 8h/day              │
└─────────────────────────────────────────────────────────────────┘
```

---

## 5. Status Bar

The status bar provides at-a-glance project state.

### Components

```
┌─────────────────────────────────────────────────────────────────┐
│ Filter: status:ready | O:15 P:3 D:8 | 45h left | @alice | ?help│
└─────────────────────────────────────────────────────────────────┘
```

| Section | Description |
|---------|-------------|
| Filter | Current active filter, or "all" |
| O/P/D | Open/In-Progress/Done counts |
| Time | Total remaining estimated hours |
| Actor | Current user (from env or config) |
| Help | Hint that `?` shows help |

### Additional Indicators

- `*` - Unsaved changes (if implementing edit)
- `~` - File watching active
- `!` - Validation warnings exist

---

## 6. Technical Approach

### 6.1 Dependencies

```toml
[dependencies]
# TUI framework (maintained fork of tui-rs)
ratatui = "0.26"

# Terminal backend
crossterm = "0.27"

# Async runtime for file watching
tokio = { version = "1", features = ["full"] }

# File watching
notify = "6"
notify-debouncer-mini = "0.4"

# Existing workgraph dependencies
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
anyhow = "1.0"
```

### 6.2 Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                         main.rs                              │
│  - Parse args (--tui flag or `wg tui` subcommand)           │
│  - Initialize terminal                                       │
│  - Run event loop                                            │
└──────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌──────────────────────────────────────────────────────────────┐
│                          App                                 │
│  - state: AppState                                           │
│  - graph: WorkGraph                                          │
│  - current_view: View                                        │
│  - filter: FilterState                                       │
│  - selection: SelectionState                                 │
└──────────────────────────────────────────────────────────────┘
                              │
              ┌───────────────┼───────────────┐
              ▼               ▼               ▼
        ┌──────────┐   ┌──────────┐   ┌──────────┐
        │  Input   │   │  Update  │   │  Render  │
        │ Handler  │   │  Logic   │   │  Views   │
        └──────────┘   └──────────┘   └──────────┘
              │               │               │
              └───────────────┼───────────────┘
                              ▼
                    ┌──────────────────┐
                    │   File Watcher   │
                    │  (async notify)  │
                    └──────────────────┘
```

### 6.3 Event Loop

```rust
// Pseudocode
async fn run_app(terminal: &mut Terminal, app: &mut App) -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(100);

    // Start file watcher
    let watcher = start_file_watcher(tx.clone())?;

    loop {
        // Render current state
        terminal.draw(|f| ui::draw(f, app))?;

        // Handle events with timeout
        tokio::select! {
            // Terminal input
            Some(event) = poll_terminal_event() => {
                match handle_input(event, app) {
                    Action::Quit => break,
                    Action::Refresh => app.reload_graph()?,
                    Action::Claim(id) => app.claim_task(&id)?,
                    Action::Done(id) => app.mark_done(&id)?,
                    // ...
                }
            }
            // File change notification
            Some(FileChanged) = rx.recv() => {
                app.reload_graph()?;
            }
        }
    }
    Ok(())
}
```

### 6.4 State Management

```rust
struct App {
    /// The workgraph data
    graph: WorkGraph,

    /// Path to .wg directory
    workgraph_dir: PathBuf,

    /// Current view mode
    view: View,

    /// Current filter
    filter: Filter,

    /// Selected task index
    selected: usize,

    /// Cached filtered/sorted task list
    visible_tasks: Vec<String>,

    /// Modal state (help, add task, etc.)
    modal: Option<Modal>,

    /// Input buffer for search/add
    input: String,

    /// Current actor (for claiming)
    actor: Option<String>,
}

enum View {
    TaskList,
    ReadyQueue,
    Graph,
}

enum Modal {
    Help,
    AddTask,
    TaskDetail(String),
    Confirm(ConfirmAction),
}

struct Filter {
    status: Option<Status>,
    assignee: Option<String>,
    tags: Vec<String>,
    search: Option<String>,
}
```

### 6.5 File Watching

Watch `.wg/graph.jsonl` for external changes (from CLI or other agents).

```rust
async fn start_file_watcher(
    tx: mpsc::Sender<AppEvent>,
    path: &Path,
) -> Result<impl Watcher> {
    let (notify_tx, notify_rx) = std::sync::mpsc::channel();

    let mut debouncer = new_debouncer(
        Duration::from_millis(200),
        notify_tx,
    )?;

    debouncer.watcher().watch(path, RecursiveMode::NonRecursive)?;

    // Spawn task to forward events
    tokio::spawn(async move {
        while let Ok(events) = notify_rx.recv() {
            let _ = tx.send(AppEvent::FileChanged).await;
        }
    });

    Ok(debouncer)
}
```

### 6.6 Integration with Existing CLI

Two options for integration:

**Option A: Subcommand**
```bash
wg tui              # Launch TUI
wg tui --watch      # Launch with file watching (default)
wg tui --no-watch   # Launch without file watching
```

**Option B: Separate binary**
```bash
wg-tui              # Separate binary, same crate
```

Recommendation: **Option A** - keeps everything in one binary, easier to install.

---

## 7. ASCII Mockups

### 7.1 Task List View

```
┌─ workgraph ─────────────────────────────────────────────────────┐
│                                                                 │
│  Tasks                                                   [?help]│
│  ──────                                                         │
│                                                                 │
│  [ ] api-design              8h   @unassigned        ready      │
│ >[~] database-work           4h   @alice             in-prog    │
│  [x] setup-ci                2h   @bob               done       │
│  [ ] frontend-components    12h   @unassigned        blocked    │
│  [ ] api-tests               6h   @unassigned        blocked    │
│  [ ] documentation           3h   @unassigned        blocked    │
│  [x] requirements-doc        2h   @carol             done       │
│  [ ] deployment-config       1h   @unassigned        ready      │
│                                                                 │
│                                                                 │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│ all | O:6 P:1 D:2 | 36h remaining | @alice       j/k:nav c:claim│
└─────────────────────────────────────────────────────────────────┘
```

### 7.2 Task Detail Modal

```
┌─ workgraph ─────────────────────────────────────────────────────┐
│                                                                 │
│  Tasks                                                          │
│  ──────                                                         │
│                                                                 │
│  [ ]┌─ database-work ────────────────────────────────────┐      │
│ >[~]│                                                    │      │
│  [x]│  Title: Database Schema Design                     │      │
│  [ ]│  ID: database-work                                 │      │
│  [ ]│  Status: in-progress                               │      │
│  [ ]│  Assigned: @alice                                  │      │
│  [x]│  Estimate: 4 hours                                 │      │
│  [ ]│  Tags: #backend #database                          │      │
│     │                                                    │      │
│     │  Created:  2024-01-14 09:00                        │      │
│     │  Started:  2024-01-15 10:30                        │      │
│     │                                                    │      │
│     │  Blocked by:                                       │      │
│     │    [x] setup-ci                                    │      │
│     │                                                    │      │
│     │  Blocks:                                           │      │
│     │    [ ] api-impl                                    │      │
│     │    [ ] data-migration                              │      │
│     │                                                    │esc   │
│     └────────────────────────────────────────────────────┘      │
├─────────────────────────────────────────────────────────────────┤
│ all | O:6 P:1 D:2 | 36h remaining | @alice              Esc:back│
└─────────────────────────────────────────────────────────────────┘
```

### 7.3 Ready Queue View

```
┌─ workgraph ─────────────────────────────────────────────────────┐
│                                                                 │
│  Ready Queue                                             [?help]│
│  ───────────                                                    │
│                                                                 │
│  Ready to work on (sorted by impact):                           │
│                                                                 │
│   #  Task                     Hours   Unblocks                  │
│  ─────────────────────────────────────────────────────────      │
│  >1. api-design                  8h   3 tasks                   │
│   2. deployment-config           1h   1 task                    │
│   3. update-readme               1h   (leaf)                    │
│                                                                 │
│  ─────────────────────────────────────────────────────────      │
│  Total ready work: 10 hours                                     │
│  Blocked tasks: 5                                               │
│                                                                 │
│                                                                 │
│                                                                 │
│                                                                 │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│ [2:Ready] | Ready:3 Blocked:5 | 10h ready | @alice   Enter:claim│
└─────────────────────────────────────────────────────────────────┘
```

### 7.4 Graph View

```
┌─ workgraph ─────────────────────────────────────────────────────┐
│                                                                 │
│  Dependency Graph: api-impl                              [?help]│
│  ──────────────────────────                                     │
│                                                                 │
│  Upstream (what api-impl depends on):                           │
│                                                                 │
│    requirements-doc [done]                                      │
│    └── api-design [open] ←── YOU ARE HERE                       │
│        └── api-impl [blocked]                                   │
│                                                                 │
│  Downstream (what depends on api-impl):                         │
│                                                                 │
│    api-impl [blocked]                                           │
│    ├── api-tests [blocked]                                      │
│    ├── frontend-integration [blocked]                           │
│    │   └── beta-release [blocked]                               │
│    └── documentation [blocked]                                  │
│        └── v1-release [blocked]                                 │
│                                                                 │
│  Critical path length: 4 tasks                                  │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│ [3:Graph] | Viewing: api-impl | Impact: 5 tasks      Tab:switch │
└─────────────────────────────────────────────────────────────────┘
```

### 7.5 Add Task Modal

```
┌─ workgraph ─────────────────────────────────────────────────────┐
│                                                                 │
│  Tasks                                                          │
│  ──────                                                         │
│                                                                 │
│  [ ] ┌─ Add Task ─────────────────────────────────────────┐     │
│ >[~] │                                                    │     │
│  [x] │  Title: _                                          │     │
│  [ ] │                                                    │     │
│  [ ] │  Blocked by: (comma-separated task IDs)           │     │
│  [ ] │  > api-design, database-work                       │     │
│  [x] │                                                    │     │
│  [ ] │  Hours: 4                                          │     │
│      │                                                    │     │
│      │  Tags: #backend                                    │     │
│      │                                                    │     │
│      │                                                    │     │
│      │                            [Cancel]  [Create]      │     │
│      └────────────────────────────────────────────────────┘     │
│                                                                 │
│                                                                 │
├─────────────────────────────────────────────────────────────────┤
│ Adding new task...                          Tab:next Esc:cancel │
└─────────────────────────────────────────────────────────────────┘
```

### 7.6 Help Overlay

```
┌─ workgraph ─────────────────────────────────────────────────────┐
│ ┌─ Help ──────────────────────────────────────────────────────┐ │
│ │                                                             │ │
│ │  Navigation                    Actions                      │ │
│ │  ──────────                    ───────                      │ │
│ │  j/↓    Move down              c    Claim task              │ │
│ │  k/↑    Move up                d    Mark done               │ │
│ │  g/Home First item             u    Unclaim                 │ │
│ │  G/End  Last item              a    Add new task            │ │
│ │  Enter  View details           r    Refresh                 │ │
│ │                                                             │ │
│ │  Views                         Search                       │ │
│ │  ─────                         ──────                       │ │
│ │  1      Task list              /    Search/filter           │ │
│ │  2      Ready queue            f    Quick filter            │ │
│ │  3      Graph view             Ctrl+l  Clear filter         │ │
│ │  Tab    Cycle views                                         │ │
│ │                                                             │ │
│ │  q/Ctrl+c  Quit                ? Toggle this help           │ │
│ │                                                             │ │
│ └─────────────────────────────────────────────────────────────┘ │
├─────────────────────────────────────────────────────────────────┤
│ Press any key to close help                                     │
└─────────────────────────────────────────────────────────────────┘
```

---

## 8. MVP Scope

### Phase 1: Core Functionality (MVP)

**Must Have:**
- [ ] Task list view with selection
- [ ] Basic navigation (j/k, Enter for details)
- [ ] Status filtering (ready, open, in-progress, done)
- [ ] Claim task (`c`)
- [ ] Mark done (`d`)
- [ ] Status bar with counts
- [ ] Help overlay (`?`)
- [ ] Quit (`q`)

**Implementation Order:**
1. Basic terminal setup with ratatui
2. Task list rendering
3. Navigation and selection
4. Task detail modal
5. Claim and done actions
6. Status bar
7. Help overlay

**Estimated Effort:** 2-3 days

### Phase 2: Enhanced Features

**Should Have:**
- [ ] Ready queue view
- [ ] Search/filter (`/`)
- [ ] Add task modal (`a`)
- [ ] File watching for live updates
- [ ] Multiple sort options
- [ ] Split pane layout option (`v`)

**Estimated Effort:** 2-3 days

### Phase 3: Advanced Features

**Nice to Have:**
- [ ] Graph visualization view
- [ ] Tag filtering
- [ ] Assignee filtering
- [ ] Keyboard shortcuts customization
- [ ] Color themes
- [ ] Mouse support

**Estimated Effort:** 3-5 days

---

## 9. Future Extensions

### 9.1 Collaborative Features

- Show which tasks are currently being worked on by other agents
- Real-time updates when another agent claims/completes tasks
- Activity log view

### 9.2 Analytics Integration

- Velocity chart (sparkline in status bar)
- Burndown visualization
- Bottleneck highlighting

### 9.3 Advanced Graph Features

- Interactive graph navigation
- Zoom/pan in graph view
- Filter graph by subgraph (show only tasks related to X)

### 9.4 Customization

- User-defined key bindings
- Custom status bar format
- Column configuration for task list
- Color schemes (dark/light/custom)

### 9.5 Integration

- tmux-style session restore
- Pipe output to other tools
- Export current view as markdown/JSON

---

## References

- [ratatui documentation](https://docs.rs/ratatui/)
- [ratatui examples](https://github.com/ratatui/ratatui/tree/main/examples)
- [crossterm documentation](https://docs.rs/crossterm/)
- [Awesome TUI](https://github.com/rothgar/awesome-tui) - inspiration gallery
- [lazygit](https://github.com/jesseduffield/lazygit) - excellent TUI UX reference
- [bottom](https://github.com/ClementTsang/bottom) - ratatui-based system monitor
