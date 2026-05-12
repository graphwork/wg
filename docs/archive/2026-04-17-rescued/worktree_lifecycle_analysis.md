# Current Worktree Lifecycle Implementation Analysis

## Overview
This document analyzes how worktrees are currently created, managed, and cleaned up in the wg system to understand where issues lie.

## 1. Worktree Creation Flow

### When agents spawn:
- Worktrees are created using `git worktree add` command
- The process is defined in `src/commands/spawn/worktree.rs`
- Key steps:
  1. Create a new git branch named `wg/{agent_id}/{task_id}` from HEAD
  2. Create worktree directory at `.wg-worktrees/{agent_id}`
  3. Symlink `.wg` into the worktree so CLI works from there
  4. Run `worktree-setup.sh` if it exists (best-effort)

### Key function: `create_worktree()`
Located in `src/commands/spawn/worktree.rs` lines 27-78

## 2. Cleanup Process

### How cleanup works:
- Called on service startup via `cleanup_orphaned_worktrees()` 
- Located in `src/commands/service/worktree.rs` lines 562-675
- Scans `.wg-worktrees/` directory for orphaned worktrees
- Compares against active agents in the registry

### Cleanup logic:
1. Read all entries in worktrees directory
2. Filter for agent directories (starting with "agent-")
3. Check if each agent is alive by:
   - Looking up agent in registry (`registry.agents.get(&name)`)
   - Checking if agent is marked as alive (`a.is_alive()`)  
   - Verifying actual process is alive (`crate::commands::is_process_alive(a.pid)`)
4. If not alive, remove the worktree using `cleanup_dead_agent_worktree()` or manual cleanup

### Key cleanup functions:
- `cleanup_orphaned_worktrees()` - main cleanup entry point (lines 562-675)
- `cleanup_dead_agent_worktree()` - enhanced cleanup with retry logic (lines 440-559)
- `remove_worktree()` - basic removal functionality (lines 81-112)

## 3. Liveness Checking Mechanism

### How liveness is determined:
- Uses both registry state and direct process checking
- In `src/commands/service/worktree.rs` line 592:
  ```rust
  .map(|a| a.is_alive() && crate::commands::is_process_alive(a.pid))
  ```
- `is_process_alive()` function in `src/service/mod.rs` lines 30-37:
  - Unix platforms: uses `kill(pid, 0)` to probe without sending signal
  - Non-unix: conservatively assumes process is alive

## 4. Race Conditions Identified

### Potential race conditions in current implementation:
1. **Process death race**: Between checking `is_alive()` and `is_process_alive()` calls, an agent could die
2. **Concurrent cleanup race**: Multiple services running concurrently could interfere with each other's worktree metadata
3. **Registry synchronization race**: Registry might be out of sync with actual processes when cleanup runs
4. **Worktree metadata race**: Global `git worktree prune` is intentionally avoided but could cause issues in concurrent scenarios

### Specific problematic areas:
- Line 592 in `src/commands/service/worktree.rs`: Combining registry check with process check can have timing issues
- Lines 631-635: Direct git commands that might conflict with concurrent operations
- The lack of atomicity in the cleanup decision process

## 5. Validation of Findings

The implementation follows this pattern:
1. Spawn creates worktree + registers agent in registry
2. Service startup runs cleanup_orphaned_worktrees
3. Cleanup compares registry state + process status to determine what to clean
4. Dead agents get their worktrees removed

## 6. Issues and Recommendations

### Current Issues:
1. **Race condition between registry and process checks** - Could mark live agents as dead
2. **No atomicity in cleanup decisions** - Multiple checks may not be consistent
3. **Inconsistent cleanup behavior** - Manual vs enhanced cleanup paths

### Areas for Improvement:
1. Implement more robust liveness checking with better error handling
2. Add atomic operations during cleanup to prevent race conditions
3. Consider more sophisticated coordination mechanisms between concurrent services
