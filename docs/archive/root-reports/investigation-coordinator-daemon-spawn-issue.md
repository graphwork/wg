# Investigation Report: Coordinator Daemon Spawning Broken

## Summary
The coordinator daemon reports `spawned=0` for 5+ consecutive ticks despite 12 tasks being in 'ready' state. Meanwhile, coordinator agents created via chat respond normally and produce tool calls. The issue is a **systemic architecture mismatch** between daemon-managed and chat/IPC-created coordinator cycles.

## Root Cause Analysis

### 1. Two Different Spawn Paths

**Daemon Path** (`src/commands/service/coordinator.rs`):
```rust
fn spawn_agents_for_ready_tasks(...) -> usize {
    // Line 3051: Skip daemon-managed loop tasks
    if is_daemon_managed(task) {
        continue;
    }
    // ... spawn logic continues
}
```

**Chat/IPC Path** (`src/commands/service/coordinator_agent.rs`):
- Creates tasks via Bash tool calling `wg add` command
- Uses `coordinator_agent` processing with full tool registry
- Bypasses daemon spawn constraints

### 2. Critical Filter: `is_daemon_managed()`

```rust
const DAEMON_MANAGED_TAGS: &[&str] = &[
    "compact-loop",      // ← THIS IS THE KEY
    "archive-loop",
    "coordinator-loop",
    "registry-refresh-loop",
    "user-board",
];

fn is_daemon_managed(task: &workgraph::graph::Task) -> bool {
    task.tags.iter().any(|tag| DAEMON_MANAGED_TAGS.contains(&tag.as_str()))
}
```

### 3. The `.compact-*` Task Problem

**Current State:**
- `.compact-0`: Status `Abandoned` (since March 28)
- `.compact-3`, `.compact-4`, `.compact-5`, etc.: Status `Open`, tagged `compact-loop`
- All have `compact-loop` tag → filtered by `is_daemon_managed()`

**Why They're Stuck:**
1. Daemon's `spawn_agents_for_ready_tasks()` skips them (line 3051)
2. Daemon's `run_graph_compaction()` only looks for `.compact-0` (lines 1656, 1690, 1790)
3. Result: They're neither spawned nor processed by compaction logic

### 4. Architecture Mismatch

**Daemon Expects**: `.compact-0` only, managed via `run_graph_compaction()`

**IPC Creates**: `.compact-N` for each `.coordinator-N` (IPC line 1187)

This creates orphaned `.compact-N` tasks that:
1. Are `Open` and `ready` 
2. Have `compact-loop` tag
3. Are skipped by both spawn and compaction paths

## Code Flow Divergence

### Daemon-Dispatched Task Spawning Path:
```
coordinator_tick() → spawn_agents_for_ready_tasks() → 
    ↓
is_daemon_managed() check → SKIPS "compact-loop" tasks →
    ↓
No spawn occurs
```

### Chat/IPC Task Spawning Path:
```
coordinator_agent processes chat → Bash tool runs `wg add` → 
    ↓
IPC handler creates tasks →
    ↓
Bypasses `is_daemon_managed()` filter →
    ↓
Tasks get assigned and spawn
```

## Specific Blocking Conditions

1. **Tag-based filtering**: `compact-loop` tags exclude tasks from normal spawn
2. **Hardcoded compaction logic**: Only `.compact-0` is processed
3. **No recovery for abandoned `.compact-0`**: Once abandoned, never recreated
4. **Multiple coordinator cycles**: IPC creates `.compact-N` but daemon only handles `.compact-0`

## System Impact

- **Compaction stalled**: At 19.7M tokens, hasn't fired in 3h
- **Backlog growing**: 12+ ready tasks not being spawned
- **Resource waste**: Ready tasks occupy slots but no agents spawned
- **Coordination failure**: Daemon can't manage its own lifecycle tasks

## Fix Directions

### Option 1: Fix Compaction Task Management
- Modify `ensure_coordinator_task()` to recreate abandoned `.compact-0`
- Update `run_graph_compaction()` to find any `.compact-*` task
- Remove "compact-loop" from `DAEMON_MANAGED_TAGS` for spawn path

### Option 2: Fix Spawn Logic for Loop Tasks
- Modify `is_daemon_managed()` to handle specific lifecycle states
- Add special case for compaction tasks that need to run
- Ensure `.compact-*` tasks get spawned when ready

### Option 3: Architectural Alignment  
- Make IPC use `.compact-0` instead of creating `.compact-N`
- Ensure single compaction task for entire system
- Clean up orphaned `.compact-N` tasks

## Recommended Immediate Fix

**Priority:** High - System functionality degraded

**Fix:** 
1. Update `ensure_coordinator_task()` to recreate `.compact-0` if abandoned
2. Modify `run_graph_compaction()` to process any `.compact-*` task (not just `.compact-0`)
3. Consider removing `compact-loop` from `DAEMON_MANAGED_TAGS` or adding exception logic

**Files to Modify:**
- `src/commands/service/mod.rs` lines 1468-1492 (`.compact-0` creation)
- `src/commands/service/mod.rs` lines 1643-1820 (`run_graph_compaction()`)
- `src/commands/service/coordinator.rs` lines 137-150 (`DAEMON_MANAGED_TAGS`)

## Verification Plan

1. After fix: Daemon should show `spawned > 0` when compaction tasks are ready
2. Compaction should fire when token threshold reached
3. All `.compact-*` tasks should progress through lifecycle
4. Backlog of ready tasks should clear

## Conclusion

The root cause is a **systemic architecture mismatch**: daemon logic expects single `.compact-0` task, while IPC creates multiple `.compact-N` tasks. These tasks are caught in a limbo: tagged as "daemon-managed" but not handled by daemon compaction logic, filtered from normal spawn path. The fix requires aligning the two code paths and fixing lifecycle management for abandoned system tasks.