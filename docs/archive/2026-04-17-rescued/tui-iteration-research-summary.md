# TUI Iteration Data Model and Tab Structure Research Summary

## Executive Summary

This research investigates how cycle iteration data is stored and displayed in the workgraph TUI, focusing on the Detail, Log, and Messages tabs. The findings reveal that only the Detail tab is currently iteration-aware, while Log and Messages tabs aggregate data across all iterations.

## Key Questions Answered

### 1. How does the graph store per-iteration data?

**Graph Structure (`src/graph.rs`)**:
- `loop_iteration: u32` - Current cycle iteration (0 = first run, incremented on re-activation)
- `last_iteration_completed_at: Option<String>` - Timestamp when most recent cycle iteration completed
- `cycle_failure_restarts: u32` - Number of failure-triggered cycle restarts consumed
- `iteration_round: u32` - Iteration tracking for which iteration round this task is 
- `iteration_anchor: Option<String>` - ID of original task this iterates from
- `iteration_parent: Option<String>` - ID of immediate prior iteration

**Archive Storage**:
- Archives stored in `.wg/log/agents/{task_id}/` as timestamp-named directories
- Each archive contains `prompt.txt`, `output.txt`/`output.log` for that iteration
- Archives sorted oldest-first by timestamp in directory name

### 2. What does the TUI currently show for tasks with multiple iterations?

**Detail Tab** - Full iteration awareness:
- Shows current iteration number from `task.loop_iteration`
- Displays "Iterations" section with browseable archive list when archives exist
- Navigation with `[` / `]` keys to browse archived iterations
- When viewing archived iteration: loads archived `prompt.txt` and `output.txt` instead of current files
- Header shows iteration label like "── task-id ── [viewing iter 2/5]"

**Log Tab** - NOT iteration-aware:
- Displays `task.log` entries from current iteration only
- No filtering or separation by iteration number
- All log entries across iterations are flattened together

**Messages Tab** - NOT iteration-aware:
- Displays all messages via `workgraph::messages::list_messages()`
- No filtering or separation by iteration number  
- All messages across iterations are aggregated together

### 3. How are the 1:Detail, 2:Log, 3:Msg tabs structured in TUI code?

**Tab Structure (`src/tui/viz_viewer/state.rs:452-464`)**:
```rust
pub enum RightPanelTab {
    Chat,      // 0
    Detail,    // 1
    Log,       // 2
    Messages,  // 3
    Agency,    // 4
    Config,    // 5
    Files,     // 6
    CoordLog,  // 7
    Firehose,  // 8
    Output,    // 9
    Dashboard, // 10
}
```

**Data Flow**:
- **Detail Tab**: Uses `app.hud_detail: Option<HudDetail>` loaded by `load_hud_detail()`
  - File: `src/tui/viz_viewer/state.rs:6164`
  - Renderer: `draw_detail_tab()` in `src/tui/viz_viewer/render.rs:2369`
  
- **Log Tab**: Uses `app.log_pane: LogPaneState` loaded by `load_log_pane()`
  - File: `src/tui/viz_viewer/state.rs:7351`  
  - Renderer: `draw_log_tab()` in `src/tui/viz_viewer/render.rs:4007`
  
- **Messages Tab**: Uses `app.messages_panel: MessagesPanelState` loaded by `load_messages_panel()`
  - File: `src/tui/viz_viewer/state.rs:7722`
  - Renderer: `draw_messages_tab()` in `src/tui/viz_viewer/render.rs:5346`

**State Management**:
- `viewing_iteration: Option<usize>` - Global TUI state for which iteration archive is being viewed
- `iteration_archives: Vec<(String, PathBuf)>` - Cached list of archived iterations for selected task
- Iteration navigation methods: `iteration_prev()`, `iteration_next()` at `src/tui/viz_viewer/state.rs:7146,7184`

### 4. What data boundaries exist between iterations?

**Current Boundaries**:
- **Task Log Entries**: `task.log` field contains log entries that span iterations (no per-iteration separation)
- **Messages**: Message queue spans iterations (no per-iteration filtering in `workgraph::messages::list_messages()`)
- **Agent Output**: Archived per-iteration in `.wg/log/agents/{task_id}/{timestamp}/output.txt`
- **Agent Prompt**: Archived per-iteration in `.wg/log/agents/{task_id}/{timestamp}/prompt.txt`

**Missing Boundaries**:
- Log entries have no `iteration_number` or `loop_iteration` field for filtering
- Messages have no iteration metadata for scoping to specific cycles
- No mechanism to separate logs by iteration in TUI display

## Feasibility Assessment: Iteration-Aware Tabs

### What can be filtered by iteration NOW:
- **Agent Output/Prompt**: Already implemented in Detail tab via archive system
- **Task metadata**: Current iteration number available in `task.loop_iteration`

### What needs new storage for iteration filtering:

**Log Entries** (High effort):
- `task.log` entries need `iteration_number` field added to `LogEntry` struct
- Existing logs would need migration or be treated as "iteration 0"
- Log loading logic needs iteration-specific filtering

**Messages** (High effort):  
- Message storage needs iteration metadata added
- `workgraph::messages` module needs iteration-aware APIs
- TUI messages loading needs iteration filtering logic

### What can be improved with current data:

**Low-hanging fruit** (Medium effort):
- Add iteration context to log/message display (e.g., show which iteration each entry belongs to)
- Add "viewing iteration X" indicator to Log/Messages tab headers when Detail tab iteration navigation is used
- Sync Log/Messages tab view state with Detail tab's `viewing_iteration`

## Implementation Path Forward

**Phase 1 - Visual Synchronization (Medium effort)**:
1. Sync Log/Messages tab state with Detail tab's `viewing_iteration`
2. Add iteration indicators to Log/Messages tab headers
3. Show contextual information about which iteration data spans

**Phase 2 - Data Model Enhancement (High effort)**:
1. Add iteration metadata to log entries and messages
2. Implement iteration-specific filtering in data loading
3. Add iteration navigation to Log/Messages tabs

**Recommended Approach**: Start with Phase 1 to provide immediate value with existing data, then evaluate if Phase 2 is worth the data migration complexity.
