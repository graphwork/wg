# Cron Triggers Design Document

## Overview

This document outlines the design for adding cron-style scheduling to workgraph, enabling time-based task triggers beyond the existing cycle iteration mechanism.

## Current State Analysis

Workgraph currently supports:
- **Cycles**: Iteration-based scheduling with `max_iterations`, restart on failure, and convergence
- **Time delays**: `delay` field in `CycleConfig` for delaying cycle re-activation
- **Task timing**: `not_before`, `ready_after` for single-time delays

**Gap**: No wall-clock scheduling for recurring operational tasks like "nightly cleanup at 2am" or "health check every 5 minutes".

## Design Goals

1. **Cron-style expressions**: Support standard cron syntax (`0 2 * * *`, `*/5 * * * *`)
2. **Coordinator integration**: Cron checking integrated into existing tick loop
3. **Task creation**: Auto-create fresh task instances on cron triggers
4. **Backward compatibility**: No breaking changes to existing functionality
5. **Conflict handling**: Handle overlapping executions gracefully

## Architecture

### 1. Data Model

Add cron-related fields to the `Task` struct in `src/graph.rs`:

```rust
/// Cron schedule expression (e.g., "0 2 * * *" for daily at 2am)
#[serde(skip_serializing_if = "Option::is_none")]
pub cron_schedule: Option<String>,

/// Whether this task has cron scheduling enabled
#[serde(default, skip_serializing_if = "is_bool_false")]
pub cron_enabled: bool,

/// Timestamp of last cron trigger (ISO 8601 / RFC 3339)
#[serde(skip_serializing_if = "Option::is_none")]
pub last_cron_fire: Option<String>,

/// Timestamp of next scheduled cron trigger (ISO 8601 / RFC 3339)
#[serde(skip_serializing_if = "Option::is_none")]
pub next_cron_fire: Option<String>,
```

### 2. Cron Module

Create `src/cron.rs` with core functionality:

```rust
use cron::Schedule;
use chrono::{DateTime, Utc};

/// Parse a cron expression and return a Schedule
pub fn parse_cron_expression(expr: &str) -> Result<Schedule, CronError>;

/// Calculate the next fire time for a schedule from a given timestamp
pub fn calculate_next_fire(schedule: &Schedule, from: DateTime<Utc>) -> Option<DateTime<Utc>>;

/// Check if a cron task is due to fire now
pub fn is_cron_due(task: &Task, now: DateTime<Utc>) -> bool;

/// Update cron timing fields after a trigger
pub fn update_cron_timing(task: &mut Task, now: DateTime<Utc>) -> Result<(), CronError>;
```

Dependencies: Add `cron = "0.12"` to `Cargo.toml`.

### 3. CLI Integration

Extend `wg add` command with `--cron` flag:

```bash
wg add "nightly cleanup" --cron "0 2 * * *" -d "Clean up old logs and temp files"
```

Implementation in `src/commands/add.rs`:
- Add `--cron <expression>` argument
- Validate cron expression during task creation
- Set `cron_enabled = true`, `cron_schedule = expression`
- Calculate and set `next_cron_fire` timestamp

### 4. Coordinator Integration

Add cron trigger checking to `coordinator_tick()` in `src/commands/service/coordinator.rs`:

```rust
fn check_cron_triggers(graph: &mut Graph, now: DateTime<Utc>) -> bool {
    let mut modified = false;
    
    for task in graph.tasks.values_mut() {
        if !task.cron_enabled || task.cron_schedule.is_none() {
            continue;
        }
        
        if is_cron_due(task, now) {
            // Create new task instance
            let new_task = create_cron_task_instance(task);
            graph.add_task(new_task);
            
            // Update timing for next fire
            update_cron_timing(task, now)?;
            modified = true;
        }
    }
    
    modified
}
```

Integration point: Add to `coordinator_tick()` in Phase 2 (before ready task checking).

### 5. Task Instance Creation

When a cron trigger fires:
1. **Create new task**: Clone the cron template task
2. **Unique ID**: Append timestamp suffix (e.g., `nightly-cleanup-2026-04-11-02-00`)
3. **Clear state**: Reset status to `Open`, clear logs/artifacts
4. **Preserve metadata**: Keep description, verify criteria, dependencies
5. **Mark template**: Update cron timing fields on the template task

### 6. Conflict Handling

**Overlapping executions**: If previous instance still running when next trigger fires:
- **Default behavior**: Create new instance anyway (parallel execution)
- **Future enhancement**: Add `cron_max_concurrent` field for limiting

**Template task management**:
- Cron template tasks remain `Open` or `Done` but never execute
- Only auto-created instances are dispatched to agents
- Template provides the blueprint for instance creation

## Implementation Plan

1. **Data model** (`design-cron-data`): Add fields to Task struct
2. **Cron parsing** (`implement-cron-parsing`): Core cron logic and module
3. **CLI support** (`add-cron-flag`): Extend `wg add` command
4. **Coordinator** (`integrate-cron-checking`): Add trigger checking to tick loop
5. **Testing** (`add-comprehensive-cron`): Comprehensive test coverage
6. **Integration** (`final-integration-and`): Final validation and documentation

## Example Usage

```bash
# Create a nightly cleanup task
wg add "nightly cleanup" --cron "0 2 * * *" \
  -d "Clean up old logs and temporary files" \
  --verify "test -f /tmp/cleanup.log"

# Create a health check every 5 minutes  
wg add "health check" --cron "*/5 * * * *" \
  -d "Check service health and alert if down"

# Start the service - coordinator will handle cron triggers
wg service start
```

## Benefits

1. **Operational tasks**: Enable scheduled maintenance, backups, health checks
2. **Automation**: Reduce manual intervention for recurring work
3. **Integration**: Leverages existing workgraph task management and agent dispatch
4. **Flexibility**: Standard cron expressions provide rich scheduling options
5. **Observability**: Task instances provide full audit trail of scheduled executions

## Future Enhancements

1. **Concurrency control**: Limit overlapping executions per cron task
2. **Time zones**: Support for timezone-aware scheduling
3. **Retry policies**: Configure retry behavior for failed cron tasks
4. **Jitter**: Add random delays to prevent thundering herd
5. **Human schedules**: Support for "business hours", "weekdays", etc.

---

*This design enables workgraph to handle both event-driven coordination and time-driven operational tasks, providing a complete task orchestration platform.*