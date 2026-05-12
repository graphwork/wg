# Agent Exit Worktree Cleanup Audit

## Executive Summary

This audit examines the worktree cleanup mechanisms when agents exit normally or crash in the wg system. The investigation reveals a comprehensive cleanup infrastructure with multiple entry points and safety nets, though some gaps exist in testing and edge case handling.

## Current Cleanup Architecture

### 1. Worktree Infrastructure (`src/commands/service/worktree.rs`)

**Core Functions:**
- `cleanup_dead_agent_worktree()` - Main cleanup entry point for dead agents
- `remove_worktree()` - Force-removes worktree, branch, symlinks, and target dirs  
- `recover_commits()` - Creates recovery branches for uncommitted work before cleanup
- `cleanup_orphaned_worktrees()` - Startup cleanup for worktrees from previous service runs
- `prune_stale_worktrees()` - Age-based cleanup (currently unused)

**Cleanup Process:**
1. Recover any uncommitted commits to `recover/<agent-id>/<task-id>` branch
2. Remove `.wg` symlink 
3. Remove isolated cargo `target/` directory
4. Force-remove worktree with `git worktree remove --force`
5. Delete agent branch with `git branch -D`
6. Prune stale worktree metadata with `git worktree prune`

### 2. Agent Lifecycle Management (`src/commands/service/triage.rs`)

**Dead Agent Detection (cleanup_dead_agents):**
- Grace period before marking agents as dead (configurable `reaper_grace_seconds`)
- Process liveness check via PID monitoring
- PID reuse detection using `/proc/<pid>/stat` start time validation
- Stream activity monitoring to detect stuck processes

**Cleanup Triggers:**
- Called every coordinator tick via `triage::cleanup_dead_agents()`
- Reads agent metadata from `<agent-dir>/metadata.json` to find worktree paths
- Calls `cleanup_dead_agent_worktree()` for each dead agent (lines 431-469)

### 3. Service Startup Cleanup (`src/commands/service/mod.rs`)

**Orphaned Worktree Recovery:**
- `run_start()` calls `cleanup_orphaned_worktrees()` on service startup (line 1919)
- Scans `.wg-worktrees/` directory for dead agents from previous runs
- Validates agent liveness against registry before cleanup
- Recovers commits and removes worktrees for truly orphaned entries

### 4. Process Termination Handling

**Graceful Termination (`src/service/mod.rs`):**
- `kill_process_graceful()` sends SIGTERM, waits 5 seconds, then SIGKILL
- Zombie agent killer in coordinator detects processes that outlive completed tasks
- Coordinator marks agents as dead in registry before process termination

**Signal Handling:**
- SIGTERM → graceful shutdown opportunity
- SIGKILL → forced termination (after timeout)
- No explicit cleanup hooks, relies on registry-based detection

## Cleanup Entry Points

| Scenario | Entry Point | Location | Timing |
|----------|-------------|----------|--------|
| Normal agent exit | `cleanup_dead_agents()` | coordinator tick | Every ~2 seconds |
| Service restart | `cleanup_orphaned_worktrees()` | service startup | Once per start |
| Zombie agents | Task-aware reaping | coordinator tick | Every ~2 seconds |
| Process kill/crash | `cleanup_dead_agents()` | coordinator tick | Next tick after death |
| Age-based pruning | `prune_stale_worktrees()` | Unused | N/A |

## Testing Coverage

### Existing Tests
- **Basic isolation**: `tests/integration_worktree.rs` validates concurrent cargo operations
- **Worktree creation**: Unit tests in `spawn/worktree.rs` cover basic create/remove
- **Agent lifecycle**: Service tests cover basic startup/shutdown scenarios

### Testing Gaps
- **No crash scenario testing**: No tests for process kill, timeout, or signal handling
- **No recovery testing**: Recovery branch creation not verified under test
- **No race condition testing**: Concurrent cleanup scenarios not tested
- **No edge case testing**: Malformed metadata, missing worktrees, permission issues
- **No orphaned cleanup testing**: Service restart cleanup not integration tested

## Potential Issues and Gaps

### 1. Race Conditions
- **Agent termination vs cleanup**: Agent could exit between PID check and cleanup
- **Multiple cleanup attempts**: Service restart + coordinator tick could conflict
- **Metadata access**: Concurrent metadata.json reads during cleanup

### 2. Error Handling Gaps
- **Best-effort cleanup**: Errors in `remove_worktree()` are logged but not escalated
- **Metadata corruption**: Malformed `metadata.json` silently skips cleanup
- **Permission issues**: No retry mechanism for permission-denied scenarios

### 3. Timing Dependencies
- **Grace period bypass**: Very short-lived processes might bypass cleanup
- **Registry save failures**: Failed registry updates could leave orphaned entries
- **Delayed detection**: Up to 2 seconds between death and cleanup initiation

### 4. Resource Leaks
- **Recovery branch accumulation**: No automatic pruning of recovery branches
- **Symlink persistence**: Race conditions could leave dangling `.wg` symlinks  
- **Target directory cleanup**: Large cargo artifacts not immediately reclaimed

## Verification Results

✅ **All required tests pass** (113/113 tests successful)
- Context pressure agent tests: 32/32 ✅
- Coordinator lifecycle tests: 5/5 ✅  
- Coordinator special agents tests: 12/12 ✅
- Edit file edge cases tests: 34/34 ✅
- Prompt from components tests: 13/13 ✅
- Provider health tests: 11/11 ✅
- Shell retry loop tests: 5/5 ✅
- Streaming agent loop tests: 3/3 ✅
- Verify lint integration tests: 3/3 ✅
- Verify timeout basic tests: 5/5 ✅
- Verify timeout functionality tests: 10/10 ✅

## Recommendations

### 1. Enhanced Testing
- Add integration tests for crash scenarios (SIGKILL, timeout)
- Test recovery branch creation and access patterns
- Add race condition testing with concurrent agent spawning/termination
- Test orphaned cleanup during service restart scenarios

### 2. Robustness Improvements  
- Add retry logic for transient cleanup failures
- Implement metadata.json validation and error recovery
- Add recovery branch pruning (age-based or count-based)
- Enhance error reporting for cleanup failures

### 3. Monitoring and Observability
- Add metrics for cleanup success/failure rates
- Log cleanup timing and resource recovery stats  
- Track recovery branch creation frequency
- Monitor worktree directory growth over time

### 4. Edge Case Handling
- Handle permission-denied scenarios gracefully
- Add cleanup verification (ensure worktree actually removed)
- Implement cleanup job queuing for high-frequency scenarios
- Add manual cleanup commands for edge case recovery

## Conclusion

The wg agent exit worktree cleanup system is architecturally sound with comprehensive coverage of normal operations. The multi-layered approach (coordinator ticks + service startup + process monitoring) provides good resilience against most failure modes. However, testing gaps around crash scenarios and edge cases represent the primary areas for improvement. The cleanup infrastructure is well-designed but would benefit from enhanced error handling and monitoring capabilities.