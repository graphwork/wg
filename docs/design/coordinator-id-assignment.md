# Coordinator ID Assignment Strategy — Design Doc

## Problem Statement

The `handle_create_coordinator` function in `src/commands/service/ipc.rs` (line 1145) does NOT check for archived or abandoned coordinators when finding the next available ID. This causes **coordinator resurrection** — when a user archives coordinators 1 and 2 and creates a new one, the system incorrectly resurrects an old coordinator ID instead of assigning a new one.

## Root Cause

```rust
// CURRENT CODE (line 1143-1149) — BUG
let mut next_id = 0u32;
loop {
    let task_id = format!(".coordinator-{}", next_id);
    if graph.get_task(&task_id).is_none() {  // ← BUG: Only checks existence
        break;                               //     Does NOT check archived/abandoned status!
    }
    next_id += 1;
}
```

Archived tasks still exist in the graph (status=Done, tag="archived"), so `graph.get_task()` returns `Some(task)` and the loop skips that ID.

## Correct Filtering (Reference Implementation)

The `handle_list_coordinators` function (lines 1390-1437) correctly filters coordinators:

```rust
// Lines 1399-1406 — CORRECT
if task.tags.iter().any(|t| t == "coordinator-loop") {
    if matches!(task.status, workgraph::graph::Status::Abandoned) {
        continue;  // ✓ Skips abandoned
    }
    if task.tags.iter().any(|t| t == "archived") {
        continue;  // ✓ Skips archived
    }
    // ... process coordinator
}
```

## Design Decision: Approach 2 (Incremented ID)

**Recommendation: Approach 2 — Spawn new coordinator with incremented ID based on existing active coordinator count.**

### Why NOT Approach 1 (Reuse coordinator-0)

- Violates user expectation: archiving "Coordinator 1" and "Coordinator 2" means those IDs are retired
- Reusing coordinator-0 after archiving it is semantically confusing
- The user's intent when archiving is to retire those coordinators, not make them available again

### Why Approach 2 (Incremented ID)

- **Semantically correct**: Archived coordinators are retired, not recycled
- **Consistent with `handle_list_coordinators`**: Uses the same filtering logic
- **Idempotent**: Multiple create operations will always produce unique IDs
- **No collisions**: Active coordinators are properly excluded from ID search
- **Backward compatible**: Doesn't change any user-facing behavior, only fixes the bug

## Algorithm for ID Assignment

Replace the existence check at line 1145 with a helper function that checks BOTH existence AND status:

```rust
// NEW HELPER FUNCTION
fn is_coordinator_slot_available(graph: &workgraph::graph::Graph, task_id: &str) -> bool {
    match graph.get_task(task_id) {
        None => true,  // Slot is empty — available
        Some(task) => {
            // If task exists but is not an active coordinator, it's available
            // Active coordinator = has "coordinator-loop" tag AND is NOT Abandoned AND is NOT archived
            if task.tags.iter().any(|t| t == "coordinator-loop") {
                // Has coordinator-loop tag — check if it's still active
                if matches!(task.status, workgraph::graph::Status::Abandoned) {
                    return true;  // Abandoned coordinator slot is available
                }
                if task.tags.iter().any(|t| t == "archived") {
                    return true;  // Archived coordinator slot is available
                }
                return false;  // Active coordinator — not available
            }
            // No coordinator-loop tag — not a coordinator, slot is available
            true
        }
    }
}
```

Then update the ID assignment loop:

```rust
let mut next_id = 0u32;
loop {
    let task_id = format!(".coordinator-{}", next_id);
    if is_coordinator_slot_available(&graph, &task_id) {
        break;
    }
    next_id += 1;
}
```

## Implementation Checklist

### Phase 1: Add Helper Function (near line 1133)

Add `is_coordinator_slot_available` function before `handle_create_coordinator`.

### Phase 2: Update ID Assignment Loop (line 1143-1149)

Replace the `is_none()` check with `is_coordinator_slot_available(&graph, &task_id)`.

### Phase 3: Verify No Other Call Sites

Check that no other code paths call `handle_create_coordinator` or similar logic that needs updating:
- `handle_delete_coordinator` (line 1241): Uses resolved ID, no ID search needed
- `handle_archive_coordinator` (line 1280): Uses explicit coordinator_id, no ID search needed
- `handle_stop_coordinator` (line 1321): Uses explicit coordinator_id, no ID search needed

### Phase 4: Add Test Coverage

Add test cases in `src/commands/service/ipc.rs` tests:
1. Create coordinator after archiving coordinator-0 → should get ID 0 (resurrect)
2. Create coordinator after archiving coordinator-1 → should get ID 1 (resurrect)
3. Create coordinator after archiving coordinator-0 and coordinator-1 → should get ID 0
4. Create multiple coordinators after archiving mixed IDs

## Backward Compatibility

- **No changes to CLI interface**: `wg coordinator create` works the same
- **No changes to IPC interface**: Response format unchanged
- **No changes to graph structure**: Tasks still have same fields
- **Only behavioral fix**: Correct coordinator ID assignment when archives exist

## File Locations

| File | Lines | Change |
|------|-------|--------|
| `src/commands/service/ipc.rs` | ~1133 (new) | Add `is_coordinator_slot_available` helper |
| `src/commands/service/ipc.rs` | 1145 | Replace `is_none()` with helper call |

## Downstream Tasks

- `implement-coordinator-id`: Implement the fix per this design
- `.flip-design-coordinator-id`: FLIP documentation for the change
