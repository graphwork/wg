# Coordinator Persistence Analysis

## Problem Statement
Service restart / TUI reload creates new coordinators instead of resuming existing ones. Users expect:
1. Coordinator state persists across service restarts and TUI reloads
2. New coordinators only created when user explicitly requests one (e.g. clicking '+')
3. TUI should restore the same coordinator tabs/layout the user had before

## Root Cause Analysis

### The Complete Flow
1. **Service Startup** (`src/commands/service/mod.rs:1933-1952`): Creates fresh `CoordinatorState` 
2. **Legacy Task Cleanup** (`src/commands/service/mod.rs:1521-1575`): Abandons ALL existing coordinator tasks
3. **TUI Startup** (`src/tui/viz_viewer/state.rs:10978-11064`): Discovers no active coordinators
4. **Auto-Creation** (`src/tui/viz_viewer/state.rs:11052`): TUI creates new coordinator via IPC
5. **Fresh Coordinator** (`src/commands/service/ipc.rs:1211-1309`): Daemon creates new task with next available ID

### Key Issues Identified

#### 1. Aggressive Legacy Cleanup (PRIMARY ISSUE)
**File:** `src/commands/service/mod.rs`, function `cleanup_legacy_daemon_tasks()` (line 1521)

```rust
let is_legacy = task.id == ".coordinator"
    || task.id.starts_with(".coordinator-")
    || task.id.starts_with(".archive-")
    || task.id.starts_with(".registry-refresh-")
    || task.id.starts_with(".user-");
if is_legacy && task.status != workgraph::graph::Status::Abandoned {
    stale_ids.push(task.id.clone());
}
```

**Problem:** The daemon marks ALL coordinator tasks as "legacy" and abandons them on startup with the message: `"Superseded by native coordinator control plane; no longer graph-managed"`. This breaks coordinator persistence entirely.

#### 2. TUI Auto-Creation Logic
**File:** `src/tui/viz_viewer/state.rs`, function `ensure_user_coordinator()` (line 11005)

```rust
let any_exist = graph.as_ref().is_some_and(|g| {
    g.tasks().any(|t| {
        t.tags.iter().any(|tag| tag == "coordinator-loop")
            && !matches!(t.status, workgraph::graph::Status::Abandoned)
            && !t.tags.iter().any(|tag| tag == "archived")
    })
});
if !any_exist {
    // No coordinators at all — create one for first-use experience
    self.create_coordinator(Some(user.clone()));
}
```

**Problem:** Since all coordinator tasks were abandoned in step 1, this check always finds no coordinators and auto-creates a new one.

#### 3. Fresh State Initialization
**File:** `src/commands/service/mod.rs` (lines 1933-1952)

```rust
let mut coord_state = CoordinatorState {
    enabled: true,
    // ... fresh values
    accumulated_tokens: CoordinatorState::load(&dir)
        .map(|cs| cs.accumulated_tokens)
        .unwrap_or(0),
    // ...
};
coord_state.save(&dir);
```

**Problem:** Creates fresh coordinator state on every startup, only preserving `accumulated_tokens`. Other state like `ticks`, `last_tick`, etc. is reset.

## Architecture Context

### Dual Coordinator System
The system has two different coordinator concepts:

1. **Coordinator Tasks** (`.coordinator-N`): Graph-managed loop tasks with `coordinator-loop` tag
   - Discovered by TUI via `list_coordinator_ids_and_labels()` 
   - Used for task-based coordination logic
   - Persisted in `graph.jsonl`

2. **Coordinator Agents**: LLM chat sessions for user interaction
   - Spawned by daemon for each coordinator ID
   - Handle chat/conversation interface
   - Managed natively by daemon

### TUI Discovery Mechanism
**File:** `src/tui/viz_viewer/state.rs`, function `list_coordinator_ids_and_labels()` (line 11099)

The TUI discovers coordinators by scanning the graph for tasks with:
- Tag: `coordinator-loop`
- Status: Not `Abandoned` 
- No `archived` tag
- ID pattern: `.coordinator-N` or `.coordinator`

## Fix Recommendations

### Primary Fix: Preserve Active Coordinator Tasks
**Modify:** `src/commands/service/mod.rs`, function `cleanup_legacy_daemon_tasks()` (line 1521)

**Current logic:** Abandons ALL coordinator tasks
**New logic:** Only abandon truly orphaned/stale coordinators

```rust
fn cleanup_legacy_daemon_tasks(dir: &Path, logger: &DaemonLogger) {
    // TODO: Instead of abandoning ALL coordinator tasks,
    // only abandon those that are truly stale:
    // 1. Check if coordinator agent is still alive
    // 2. Check if coordinator has been inactive for extended period
    // 3. Preserve coordinators that were recently active or have ongoing chat sessions
    // 4. Add configuration option to control this behavior
}
```

### Secondary Fix: Improve State Persistence
**Enhance:** `CoordinatorState` loading to preserve more fields across restarts:
- `ticks` count
- `last_tick` timestamp  
- `paused` status
- Other operational state

### Configuration Option
Add config setting to control coordinator persistence behavior:
```toml
[coordinator]
preserve_on_restart = true  # Default: true (preserve existing coordinators)
cleanup_stale_after_hours = 24  # Only cleanup coordinators inactive for N hours
```

## Impact Assessment

**Current Behavior:** New coordinator created on every restart
**Fixed Behavior:** Coordinators persist across restarts, TUI shows existing tabs
**Migration:** Existing behavior preserved if `preserve_on_restart = false`

## Test Verification

To verify the fix works:
1. Start service, create coordinator(s) via TUI  
2. Stop service
3. Restart service
4. Launch TUI
5. **Expected:** Same coordinators visible, no new ones created
6. **Current:** Fresh coordinator created, old ones gone

## Related Files

### Core Files
- `src/commands/service/mod.rs` - Service startup, coordinator state, cleanup logic
- `src/commands/service/ipc.rs` - Coordinator creation/deletion IPC handlers  
- `src/tui/viz_viewer/state.rs` - TUI coordinator discovery and management
- `src/service/coordinator_cycle.rs` - Coordinator task validation

### State Files
- `.workgraph/service/coordinator-state-N.json` - Per-coordinator runtime state
- `.workgraph/service/registry.json` - Agent registry tracking coordinator agents
- `.workgraph/graph.jsonl` - Task graph containing coordinator tasks
- `.workgraph/tui-state.json` - TUI session state (active coordinator, tabs)