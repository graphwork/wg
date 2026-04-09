# Safe Coordinator Cycle Design

## Problem Statement

Coordinator-22's compaction cycle was broken in 3 ways:
1. **Chat-driven**: Compaction was triggered by chat messages instead of the cycle
2. **Circular archive deadlock**: Archive was added as a dependency, creating a cycle
3. **No context injection**: Compaction output wasn't injected into coordinator context

These issues were only discovered after iteration 15. We need to codify safe defaults so this can't happen again.

## Safe Coordinator Cycle Structure

### Correct Dependency Graph

```
.coordinator-N → .compact-N → .coordinator-N (cycle: OK)
.archive-N    → (runs independently, NOT gated by coordinator)
```

### Key Rules

1. **Coordinator → Compact**: Sequential, coordinator waits for compact to complete
2. **Compact → Coordinator**: Sequential, compact task completes → coordinator re-activates
3. **Archive runs independently**: On schedule/threshold, NOT gated by coordinator
4. **NO circular coordinator↔archive dependency**: This creates deadlock

### Why Archive Must Be Independent

If coordinator has `.archive-N` in its `after` list:
```
.coordinator → .archive → .coordinator (DEADLOCK)
```

The coordinator waits for archive, but archive waits for coordinator (implicitly or via cycle). Archive should run on:
- Time-based schedule (e.g., every 24 hours)
- Threshold-based (e.g., >100 done/abandoned tasks)
- NOT as a blocker on coordinator iteration

## Safe Defaults

### CoordinatorConfig

| Setting | Safe Default | Rationale |
|---------|-------------|----------|
| `heartbeat_interval` | 60 (seconds) | Prevents coordinator from stalling if no user input |
| `max_agents` | 8 | Limits concurrent agents to prevent resource exhaustion |
| `eval_frequency` | "every_5" | Balance between evaluation overhead and quality monitoring |
| `compactor_interval` | 5 | Run compaction every 5 coordinator ticks |
| `compactor_ops_threshold` | 100 | Trigger compaction after 100 provenance ops |
| `compaction_token_threshold` | 100_000 | Trigger compaction at ~100k tokens accumulated |
| `compaction_threshold_ratio` | 0.8 | Trigger at 80% of context window |

### CycleConfig (for coordinator tasks)

| Setting | Safe Default | Rationale |
|---------|-------------|----------|
| `max_iterations` | 0 (unlimited) | Coordinator should run indefinitely |
| `no_converge` | true | Coordinator cannot signal convergence |
| `restart_on_failure` | true | Restart cycle on any failure |
| `max_failure_restarts` | 3 | Prevent infinite failure loops |

## Validation Rules

### 1. Detect Circular Coordinator↔Archive Dependencies

When a coordinator task is created or modified, validate:
- If task has `coordinator-loop` tag
- And task has archive task in `after` list
- Then ERROR: "Coordinator cannot depend on archive — creates circular deadlock"

### 2. Warn on heartbeat_interval = 0

When coordinator config has `heartbeat_interval = 0`:
- WARN if no explicit trigger mechanism (no user board, no scheduled trigger)
- The coordinator will only run on GraphChanged events or manual messages
- This is valid for reactive-only mode but risky for autonomous operation

### 3. Verify Context Injection Path

For coordinator to receive compaction output:
- Verify `.compactor/context.md` exists and is readable
- Verify compact task completes and produces context.md
- Verify coordinator prompt assembly includes context.md

## Implementation

### Validation Functions

Located in `src/service/coordinator_cycle.rs`:

```rust
/// Validate coordinator cycle structure
pub fn validate_coordinator_cycle(
    graph: &WorkGraph,
    coordinator_id: &str,
) -> Vec<CoordinatorCycleWarning> {
    let mut warnings = Vec::new();
    
    // Check for circular coordinator↔archive dependency
    if let Some(warning) = check_circular_archive_dependency(graph, coordinator_id) {
        warnings.push(warning);
    }
    
    // Check heartbeat_interval configuration
    if let Some(warning) = check_heartbeat_configuration(graph, coordinator_id) {
        warnings.push(warning);
    }
    
    // Check context injection path
    if let Some(warning) = check_context_injection_path(graph, coordinator_id) {
        warnings.push(warning);
    }
    
    warnings
}
```

### Where Validation Runs

1. **On coordinator creation** (IPC: `HandleCreateCoordinator`)
   - Validate the cycle structure before returning success
   
2. **On coordinator tick** (daemon coordinator.rs)
   - Log warnings but don't block (daemon must be resilient)
   
3. **On `wg check`** command
   - Return exit code != 0 if critical issues found

## Regression Test

### Test: Safe Coordinator Cycle Pattern

```
Test: coordinator_with_compact_archive_cycle
1. Create coordinator with compact and archive tasks
2. Verify NO circular coordinator↔archive dependency
3. Verify heartbeat_interval > 0
4. Verify context injection path exists (compact task → context.md → coordinator)
```

## Pattern Keywords

- **autopoietic**: Tasks self-organize via cycle edges
- **loop/cycle**: Coordinator-Compact form an infinite iteration cycle
- **regression**: Prevents reintroduction of known-broken patterns

## References

- Coordinator tick logic: `src/commands/service/coordinator.rs`
- IPC handlers: `src/commands/service/ipc.rs`
- Compactor: `src/service/compactor.rs`
- Cycle detection: `src/graph.rs` → `WorkGraph::compute_cycle_analysis()`
