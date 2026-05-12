# Task Unclaiming on Agent Death - Verification Report

## Overview

This document verifies that when an agent process dies unexpectedly (crash, kill, timeout), its claimed task is properly released back to 'open' state for re-dispatch.

## Task Unclaiming Flow and Triggers

### 1. Primary Flow: cleanup_dead_agents()

**Location:** `src/commands/service/triage.rs:174`

**Called by:** Coordinator tick cycle in `src/commands/service/coordinator.rs:49`

**Trigger:** Every coordinator tick (~periodic, coordinator's main loop)

**Flow:**
1. **Dead Agent Detection:** 
   - `detect_dead_reason()` checks each agent:
     - Process no longer exists (`!is_process_alive(agent.pid)`)
     - PID reused by different process (`verify_process_identity()` fails)
     - Agent marked as Dead but not yet cleaned up
   - Grace period enforced (`reaper_grace_seconds` config)

2. **Agent Cleanup:**
   - Mark dead agents as `AgentStatus::Dead`
   - Set `completed_at` timestamp
   - Save registry changes

3. **Task Unclaiming:**
   - For each dead agent with task still `Status::InProgress`:
     - **Auto-triage** (if enabled): Run LLM assessment of work progress
       - If verdict="done": Mark task complete based on agent output
       - If triage fails: Fall back to unclaim behavior
     - **Standard unclaim** (if auto-triage disabled):
       - Set `task.status = Status::Open`
       - Set `task.assigned = None` 
       - Increment `task.retry_count`
       - Escalate model tier if retry count warrants
       - Log unclaim reason with agent details

### 2. Secondary Flow: reconcile_orphaned_tasks()

**Location:** `src/commands/sweep.rs:232`

**Called by:** Coordinator tick after cleanup_dead_agents() in `src/commands/service/coordinator.rs:60`

**Purpose:** Safety net for split-save race conditions

**Trigger:** Every coordinator tick (as fallback)

**Flow:**
1. **Orphaned Task Detection:**
   - Find tasks with `Status::InProgress`
   - Check if assigned agent is Dead in registry OR not alive
   - Special handling for missing agents (5+ minutes timeout)
   - Exclude coordinator/compact loop tasks

2. **Task Recovery:**
   - Set `task.status = Status::Open`
   - Set `task.assigned = None`
   - Log reconciliation message
   - Return count of recovered tasks

## Death Detection Scenarios Tested

### 1. Process Exit Detection
- **Test:** `test_dead_detection_process_exited()` 
- **Scenario:** Agent marked Working but PID 999999999 doesn't exist
- **Result:** ✅ Detected as dead, task unclaimed

### 2. Daemon Restart Scenario  
- **Test:** `test_dead_agent_detection_after_daemon_restart()`
- **Scenario:** Registry has agent with non-existent PID (daemon restart)
- **Result:** ✅ Old PID detected as dead, ready for cleanup

### 3. Already Dead Agents
- **Test:** `test_dead_detection_ignores_already_dead_agents()`
- **Result:** ✅ Agents already marked Dead are ignored (no re-processing)

### 4. Process Still Running
- **Test:** `test_dead_detection_process_still_running()`
- **Scenario:** Agent with current process PID (still alive)
- **Result:** ✅ Not detected as dead, task remains claimed

### 5. Split-Save Race Condition
- **Test:** `test_reconcile_orphaned_tasks()`
- **Scenario:** Task InProgress but agent Dead in registry
- **Result:** ✅ Reconciliation recovers task to Open state

### 6. Slot Accounting 
- **Test:** `test_slot_accounting_with_dead_agents()`
- **Result:** ✅ Dead agents don't count toward max_agents limit

## Task State Transitions Verified

### Normal Agent Death Flow:
1. **Working Agent:** `Status::InProgress` + `assigned="agent-id"`
2. **Process Dies:** PID no longer exists
3. **Detection:** `cleanup_dead_agents()` finds dead process
4. **Agent Marked:** `AgentStatus::Dead` in registry
5. **Task Unclaimed:** `Status::Open` + `assigned=None` + retry increment
6. **Ready for Redispatch:** Task appears in `wg ready` for re-assignment

### Emergency Recovery Flow:
1. **Orphaned Task:** `Status::InProgress` but agent Dead in registry
2. **Detection:** `reconcile_orphaned_tasks()` safety net
3. **Recovery:** Force `Status::Open` + `assigned=None`
4. **Logging:** "Reconciliation: task recovered from orphaned state"

## Timing and Configuration

### Grace Period
- **Config:** `agent.reaper_grace_seconds` (default varies)
- **Purpose:** Avoid false positives during brief restarts
- **Implementation:** `detect_dead_reason()` checks agent age against grace

### Coordinator Tick Frequency
- **Primary cleanup:** Every coordinator tick
- **Reconciliation:** Every coordinator tick (as safety net)
- **No permanent locks:** Tasks cannot be permanently stuck

### Auto-Triage Integration
- **Enabled via:** `config.agency.auto_triage = true`
- **Benefit:** Smart completion detection vs blind restart
- **Fallback:** If triage fails, reverts to standard unclaim

## Test Coverage Summary

### Unit Tests (Sweep)
- ✅ `test_reconcile_orphaned_tasks()` - Recovery from registry inconsistency
- ✅ `test_sweep_idempotent()` - Multiple sweeps don't conflict
- ✅ `test_find_orphaned_tasks_no_agent()` - Missing agent detection

### Integration Tests (Service Coordinator)
- ✅ `test_dead_agent_detection_after_daemon_restart()` - Post-restart cleanup
- ✅ `test_dead_detection_process_exited()` - Process death detection  
- ✅ `test_slot_accounting_with_dead_agents()` - Resource counting
- ✅ `test_registry_mark_as_dead()` - Agent state transitions

### Additional Tests
- ✅ `test_concurrent_head_reference()` - Concurrent git operations
- ✅ `test_coordinator_lifecycle()` - Coordinator management

## Validation Results

### ✅ Documented task unclaiming flow and triggers
- Comprehensive flow documented for both primary and safety net paths
- Triggers clearly identified (coordinator tick, process death detection)

### ✅ Tested death scenarios and verified task state transitions  
- Multiple death scenarios covered by existing test suite
- State transitions verified: InProgress → Open, assigned → None
- Both normal and edge case handling tested

### ✅ Confirmed tasks return to open/ready state for re-dispatch
- Task status correctly reset to `Status::Open`
- Assignment cleared (`assigned = None`) 
- Tasks appear in `wg ready` output for coordinator redispatch

### ✅ No tasks permanently stuck in claimed state after agent death
- Dual safety mechanisms: cleanup_dead_agents() + reconcile_orphaned_tasks()
- Both run every coordinator tick (no periodic gaps)
- Tests verify idempotent operation (can run repeatedly safely)
- Grace period prevents false positives but ensures eventual cleanup

## Configuration Recommendations

1. **Monitor grace period:** Tune `agent.reaper_grace_seconds` based on environment
2. **Enable auto-triage:** Set `config.agency.auto_triage = true` for smarter recovery
3. **Coordinator tick rate:** Default coordinator timing handles most scenarios well
4. **Test coverage:** Existing test suite provides comprehensive coverage

## Conclusion

The wg task unclaiming system is **robust and well-tested**. Agent death detection and task unclaiming work correctly across multiple scenarios including process exits, daemon restarts, and race conditions. The dual safety mechanism (primary cleanup + reconciliation fallback) ensures no tasks can be permanently stuck in claimed state after agent death.