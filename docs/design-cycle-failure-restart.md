# Design: Cycle Failure Restart Semantics

**Date:** 2026-03-01
**Status:** Draft — awaiting review
**Task:** cycle-failure-restart

---

## Problem

When a task in a cycle fails, the cycle dead-ends. The `reactivate_cycle()` function
(`src/graph.rs:1118`) checks that **all** cycle members have status `Done` before
re-activating the cycle for its next iteration. If any member is `Failed`, the check
fails and the cycle never restarts.

This defeats the purpose of cycles for CI-retry workflows. In a cycle like
`commit → fix-ci → verify → (back to commit)`, if `verify` fails, the cycle stops.
But the intent is "keep going until green" — failure at any point should mean
"try again from the top."

### Current behavior trace

1. Cycle: `A → B → C → A` with `max_iterations: 5` on A.
2. A completes (Done), B completes (Done), C fails (Failed).
3. `reactivate_cycle()` is called (either by `evaluate_cycle_iteration` from `wg done`
   on the last completing member, or by the coordinator's phase 2.5 sweep).
4. Line 1126-1128: checks each member — finds C is `Failed`, not `Done`.
5. Returns `vec![]` — no reactivation. Cycle is permanently stuck.
6. Downstream tasks of C are unblocked (because `Failed` is terminal via `is_terminal()`),
   but the cycle itself never iterates again.

### Why this matters

The semantics of `Failed` in the cycle context are ambiguous:
- For **forward edges**, `Failed` is terminal — it "satisfies" the dependency
  (downstream isn't blocked by it, per `is_blocker_satisfied`).
- For **cycle iteration**, `Failed` is treated as "not done" — but there's no
  mechanism to restart from the failure, creating a dead end.

---

## Design Principles

1. **Opt-in.** Cycle failure restart must be explicitly enabled. Existing cycles that
   don't set the flag retain current behavior (failure = dead end).
2. **Bounded.** Failure restarts count against a separate retry budget to prevent
   infinite failure loops. This is distinct from `max_iterations` (which bounds
   successful iteration).
3. **Observable.** Every failure-triggered restart is logged with the failed task ID,
   failure reason, and retry count.
4. **Composable.** Works correctly with `--no-converge`, `--max-iterations`, guards,
   and delays.
5. **Simple.** Minimal new concepts — extends `CycleConfig` with two fields.

---

## Design

### New fields on `CycleConfig`

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CycleConfig {
    pub max_iterations: u32,
    pub guard: Option<LoopGuard>,
    pub delay: Option<String>,
    pub no_converge: bool,

    // --- NEW ---

    /// When true, if any cycle member is Failed, restart the entire cycle
    /// from the header (re-open all members) instead of dead-ending.
    #[serde(default, skip_serializing_if = "is_false")]
    pub restart_on_failure: bool,

    /// Maximum number of failure-triggered restarts per cycle lifetime.
    /// Prevents infinite failure loops. Defaults to 3 if restart_on_failure
    /// is true and this field is None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_failure_restarts: Option<u32>,
}
```

### New field on `Task`

```rust
pub struct Task {
    // ... existing fields ...

    /// Number of failure-triggered cycle restarts consumed (distinct from retry_count,
    /// which tracks individual task retries).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cycle_failure_restarts: u32,
}
```

The `cycle_failure_restarts` counter lives on the cycle config owner task (the one with
`cycle_config`). It counts how many times the cycle has been restarted due to failure,
across all iterations. This is distinct from:
- `retry_count` — per-task, counts individual task retries (via `wg retry`).
- `loop_iteration` — counts successful cycle iterations.

### CLI interface

```bash
# Enable failure restart with default max (3)
wg add "verify" --after fix-ci --max-iterations 5 --restart-on-failure

# Enable with custom max failure restarts
wg add "verify" --after fix-ci --max-iterations 5 --restart-on-failure --max-failure-restarts 10

# Modify an existing cycle header
wg edit verify --restart-on-failure --max-failure-restarts 5
```

`--restart-on-failure` requires `--max-iterations` (same validation pattern as
`--no-converge`).

---

## Detailed Semantics

### 1. Should cycle failure automatically restart from the header? Or require explicit retry?

**Decision: Automatic, opt-in via `--restart-on-failure` flag.**

**Rationale:** The whole point of CI-retry cycles is automation. Requiring manual `wg retry`
for each failure in a cycle defeats the purpose. But because not all cycles want this
(some cycles should dead-end on failure, e.g., a review-revise loop where failure means
"this approach won't work"), it must be opt-in.

When `restart_on_failure` is true and a cycle member is Failed, the cycle restarts:
- All cycle members are reset to `Open`.
- `assigned`, `started_at`, `completed_at` are cleared.
- `loop_iteration` is **not** incremented (the failed iteration doesn't count as a
  completed iteration).
- `cycle_failure_restarts` is incremented on the config owner.
- The failed task's `failure_reason` is preserved in a log entry before clearing.

When `restart_on_failure` is false (default), behavior is unchanged — failure dead-ends
the cycle.

### 2. Should there be a max-failure-retries separate from max-iterations?

**Decision: Yes. `max_failure_restarts` is separate from `max_iterations`.**

**Rationale:** These count different things:

- `max_iterations` bounds how many times the cycle completes successfully. A CI-retry
  cycle with `max_iterations: 10` means "try up to 10 green runs." This is the
  convergence/completeness bound.
- `max_failure_restarts` bounds how many times the cycle can restart due to failure.
  This is the failure-tolerance bound.

They serve different purposes and exhausting one should not exhaust the other.
If a cycle with `max_iterations: 5, max_failure_restarts: 3` runs as:
  iter0 → fail → restart (restart 1/3)
  iter0 → fail → restart (restart 2/3)
  iter0 → success → iter1 (iteration 1/5)
  iter1 → success → iter2 (iteration 2/5)
  ...
The iteration counter only increments on success; the restart counter only increments on
failure.

**Default:** When `restart_on_failure` is true and `max_failure_restarts` is None,
the effective limit is 3. This prevents runaway failure loops while being generous
enough for transient failures.

### 3. When restarting after failure, should the failed task be marked as retried? Or does the whole cycle iteration count as failed?

**Decision: The whole cycle iteration is restarted. Individual task `retry_count` is not incremented.**

**Rationale:** The cycle is the unit of retry, not the individual task. When a cycle
restarts due to failure:

- All members are reset to `Open` (not just the failed one).
- `loop_iteration` stays the same (the iteration was not completed).
- Individual tasks' `retry_count` is **not** incremented (that counter is for
  `wg retry`, which is a different mechanism — manual, per-task retry).
- The `cycle_failure_restarts` counter on the config owner is incremented.

This means the failed task gets a fresh start along with all other cycle members.
The log entries provide the full history of what happened.

**What gets preserved across failure restarts:**
- Task logs (full history).
- `cycle_failure_restarts` counter.
- `loop_iteration` (stays at the same value — the failed iteration is retried).

**What gets cleared:**
- `status` → `Open` (all members).
- `assigned` → `None` (all members).
- `started_at`, `completed_at` → `None` (all members).
- `failure_reason` → `None` (on the failed member).
- `converged` tag → removed (on config owner, if present).

### 4. How does this interact with --no-converge? With --max-iterations?

**`--max-iterations` interaction:**

- `max_iterations` bounds completed iterations, not failure restarts.
- A failure restart does not increment `loop_iteration`.
- If `loop_iteration >= max_iterations`, the cycle is exhausted regardless of
  `restart_on_failure`. But this condition is only checked on successful completion
  (the "all Done" path), so it doesn't conflict.
- If `max_failure_restarts` is exhausted, the cycle dead-ends with the failed task
  left in `Failed` state, even if `max_iterations` hasn't been reached.

**`--no-converge` interaction:**

- `--no-converge` prevents agents from signaling convergence (ignores the `converged`
  tag). This is about successful iterations.
- Failure restart is orthogonal: `--no-converge` doesn't affect whether failures
  trigger restarts. A cycle can have both `--no-converge` (must run all N iterations)
  and `--restart-on-failure` (retry on failure).
- When restarting after failure, the `converged` tag is cleared regardless of
  `--no-converge` (it would have been stale anyway from a failed iteration).

**Guard interaction:**

- Guards (`LoopGuard`) are evaluated on successful completion to decide whether to
  iterate. They are not consulted during failure restart.
- This is intentional: a guard like `TaskStatus { task: "tests", status: Done }` checks
  a condition for continuing iteration. On failure, the question isn't "should we
  continue iterating?" but "should we retry the failed iteration?" — which is answered
  by `restart_on_failure` and `max_failure_restarts`.

**Delay interaction:**

- When restarting after failure, the `delay` from `CycleConfig` is applied (if set).
  This prevents rapid-fire failure retries.
- The delay sets `ready_after` on the config owner task, just like in normal iteration.

### 5. Should the cycle header config gain a --restart-on-failure flag?

**Decision: Yes.** Two new fields on `CycleConfig`:
- `restart_on_failure: bool` (default `false`)
- `max_failure_restarts: Option<u32>` (default effective value: 3 when restart_on_failure is true)

And two new CLI flags on `wg add`:
- `--restart-on-failure`
- `--max-failure-restarts <N>`

Both require `--max-iterations` (same validation as `--no-converge`).

---

## Implementation Plan

### Phase 1: Data model changes

**Files:** `src/graph.rs`

1. Add `restart_on_failure: bool` and `max_failure_restarts: Option<u32>` to `CycleConfig`.
2. Add `cycle_failure_restarts: u32` to `Task`.
3. Modify `reactivate_cycle()` to handle the failure case:

```rust
fn reactivate_cycle(
    graph: &mut wg,
    members: &[String],
    config_owner_id: &str,
    cycle_config: &CycleConfig,
) -> Vec<String> {
    // Check if ALL members are Done (existing happy path)
    let all_done = members.iter().all(|id| {
        graph.get_task(id).map(|t| t.status == Status::Done).unwrap_or(false)
    });

    if all_done {
        // ... existing iteration logic (unchanged) ...
    }

    // NEW: Check for failure restart
    if !cycle_config.restart_on_failure {
        return vec![];
    }

    // Check if any member is Failed (and rest are Done or Failed)
    let any_failed = members.iter().any(|id| {
        graph.get_task(id).map(|t| t.status == Status::Failed).unwrap_or(false)
    });
    let all_terminal = members.iter().all(|id| {
        graph.get_task(id).map(|t| t.status.is_terminal()).unwrap_or(false)
    });

    if !any_failed || !all_terminal {
        return vec![]; // Not all finished yet, or no failures
    }

    // Check max_failure_restarts
    let failure_restarts = graph
        .get_task(config_owner_id)
        .map(|t| t.cycle_failure_restarts)
        .unwrap_or(0);
    let max_failure_restarts = cycle_config.max_failure_restarts.unwrap_or(3);
    if failure_restarts >= max_failure_restarts {
        return vec![]; // Exhausted failure restart budget
    }

    // Collect failure info for logging before mutating
    let failed_tasks: Vec<(String, Option<String>)> = members.iter()
        .filter_map(|id| {
            graph.get_task(id).and_then(|t| {
                if t.status == Status::Failed {
                    Some((id.clone(), t.failure_reason.clone()))
                } else {
                    None
                }
            })
        })
        .collect();

    // Compute delay
    let ready_after = cycle_config.delay.as_ref().and_then(|d| {
        parse_delay(d).and_then(|secs| {
            if secs <= i64::MAX as u64 {
                Some((Utc::now() + Duration::seconds(secs as i64)).to_rfc3339())
            } else {
                None
            }
        })
    });

    // Re-open all members
    let current_iter = graph
        .get_task(config_owner_id)
        .map(|t| t.loop_iteration)
        .unwrap_or(0);
    let new_failure_restarts = failure_restarts + 1;

    let mut reactivated = Vec::new();

    for member_id in members {
        if let Some(task) = graph.get_task_mut(member_id) {
            task.status = Status::Open;
            task.assigned = None;
            task.started_at = None;
            task.completed_at = None;
            task.failure_reason = None;
            // loop_iteration stays the same — this is a retry of the same iteration
            if *member_id == config_owner_id {
                task.ready_after = ready_after.clone();
                task.cycle_failure_restarts = new_failure_restarts;
                task.tags.retain(|t| t != "converged");
            }

            let failed_info: Vec<String> = failed_tasks.iter()
                .map(|(id, reason)| {
                    match reason {
                        Some(r) => format!("{}: {}", id, r),
                        None => id.clone(),
                    }
                })
                .collect();

            task.log.push(LogEntry {
                timestamp: Utc::now().to_rfc3339(),
                actor: None,
                message: format!(
                    "Cycle failure restart {}/{} (iteration {}). Failed: [{}]",
                    new_failure_restarts, max_failure_restarts,
                    current_iter, failed_info.join(", ")
                ),
            });

            reactivated.push(member_id.clone());
        }
    }

    reactivated
}
```

The key structural change is that `reactivate_cycle` now has two paths:
1. **All Done → iterate** (existing, increments `loop_iteration`).
2. **Any Failed + all terminal + restart_on_failure → failure restart** (new, increments
   `cycle_failure_restarts`, does not increment `loop_iteration`).

### Phase 2: CLI changes

**Files:** `src/main.rs`, `src/commands/add.rs`

1. Add `--restart-on-failure` and `--max-failure-restarts <N>` flags to `wg add`.
2. Validate that `--restart-on-failure` requires `--max-iterations`.
3. Pass through to `CycleConfig` construction.

### Phase 3: Coordinator changes

**Files:** `src/commands/service/coordinator.rs`

No changes needed. The coordinator already calls `evaluate_all_cycle_iterations()` in
its phase 2.5 sweep, which calls `reactivate_cycle()` for each cycle. The modified
`reactivate_cycle()` handles the failure case transparently.

However, there is a timing consideration: the coordinator sweep runs when "all members
are Done" — but for failure restart, we need to detect "all members are terminal
(Done or Failed)." The existing sweep in `evaluate_all_cycle_iterations` already calls
`reactivate_cycle()` per cycle, so the new failure-restart path will be checked there.

The only adjustment: `evaluate_all_cycle_iterations` currently skips cycles without a
`CycleConfig`. With failure restart, it still only fires for cycles with config, so
no change needed.

### Phase 4: Display changes

**Files:** `src/commands/cycles.rs`, `src/commands/show.rs`

1. In `wg cycles`, show failure restart count alongside iteration count.
2. In `wg show`, display `cycle_failure_restarts` if non-zero.

### Phase 5: Tests

**Files:** `tests/integration_cycle_detection.rs`

New test cases:

1. `test_failure_restart_reactivates_cycle` — one member Failed, restart_on_failure=true
   → all members re-opened.
2. `test_failure_restart_disabled_by_default` — one member Failed, no restart_on_failure
   → cycle dead-ends (existing behavior preserved).
3. `test_failure_restart_max_exceeded` — failure restarts exhausted → cycle dead-ends.
4. `test_failure_restart_preserves_iteration` — `loop_iteration` stays the same after
   failure restart.
5. `test_failure_restart_increments_counter` — `cycle_failure_restarts` incremented.
6. `test_failure_restart_with_delay` — delay applied on failure restart.
7. `test_failure_restart_clears_failure_reason` — failed member's `failure_reason`
   cleared on restart.
8. `test_failure_restart_with_no_converge` — both flags work together.
9. `test_failure_restart_partial_failure` — only some members Failed, rest still
   in-progress → no restart yet (must wait for all to be terminal).
10. `test_failure_restart_then_successful_iteration` — failure restart followed by
    successful completion → normal iteration continues.

---

## Decision Summary

| Question | Decision | Rationale |
|----------|----------|-----------|
| Automatic or manual restart? | Automatic, opt-in via `--restart-on-failure` | CI-retry cycles need automation; opt-in preserves safety |
| Separate max-failure-retries? | Yes, `max_failure_restarts` (default 3) | Different semantic from `max_iterations`; independent budgets |
| Per-task or per-cycle retry? | Per-cycle (whole iteration restarts) | Cycle is the unit of work; individual `retry_count` is for `wg retry` |
| Interaction with --no-converge? | Orthogonal; both can be set | --no-converge is about convergence signaling; failure restart is about error recovery |
| New flag on CycleConfig? | Yes: `restart_on_failure` + `max_failure_restarts` | Minimal extension of existing config pattern |

---

## Risks and Mitigations

### Risk: Infinite failure loops

**Mitigation:** `max_failure_restarts` with a conservative default of 3. When exhausted,
the cycle dead-ends with the Failed task left in its failed state. Operator can inspect
logs, fix the issue, and `wg retry` the failed task manually (which would need awareness
of the cycle context — see follow-up below).

### Risk: Confusing interaction between retry_count and cycle_failure_restarts

**Mitigation:** These are clearly separate concepts:
- `retry_count` = individual task retries via `wg retry`.
- `cycle_failure_restarts` = cycle-level failure restarts, only on config owner.
Documentation and log messages make the distinction clear.

### Risk: Race between triage and failure restart

When an agent dies and triage marks the task as Failed, the coordinator's next tick
sweep will see the Failed member and (if restart_on_failure is true) restart the cycle.
This is the desired behavior — triage → fail → cycle restart. No race: triage runs
during `cleanup_dead_agents` (phase 1 of the tick), cycle sweep runs in phase 2.5.

### Risk: Multiple members fail simultaneously

If multiple cycle members fail in the same iteration, the failure restart still works
correctly: `all_terminal` checks that all members are either Done or Failed, `any_failed`
confirms at least one failed. All members are restarted together. Log entries list all
failed members.

---

## Follow-up Work

1. **`wg retry` cycle awareness** — When `wg retry` is called on a Failed task that is
   a cycle member with `restart_on_failure`, should it restart the whole cycle or just
   the individual task? Currently it just reopens the individual task, which may leave
   the cycle in an inconsistent state (one member Open, others Done). Consider adding
   `--cycle` flag to `wg retry`.

2. **Metrics/observability** — Add `cycle_failure_restarts` to `wg status` summary and
   TUI dashboard for at-a-glance visibility into cycle health.

3. **Exponential backoff** — Consider doubling the delay on each consecutive failure
   restart (when `delay` is set). First failure restart uses configured delay, second
   uses 2x, etc. Useful for transient external failures.

---

## Appendix: Component Mapping

| Component | File | Change Type |
|-----------|------|-------------|
| CycleConfig struct | `src/graph.rs:8-20` | Add 2 fields |
| Task struct | `src/graph.rs:150-240` | Add 1 field |
| reactivate_cycle() | `src/graph.rs:1118-1205` | Major modification |
| evaluate_all_cycle_iterations() | `src/graph.rs:1216-1246` | No change needed |
| CLI flags | `src/main.rs` | Add 2 flags |
| wg add handler | `src/commands/add.rs:180-210` | Pass new fields |
| wg cycles display | `src/commands/cycles.rs` | Show failure restart info |
| wg show display | `src/commands/show.rs` | Show cycle_failure_restarts |
| Coordinator tick | `src/commands/service/coordinator.rs:1048-1065` | No change needed |
| Integration tests | `tests/integration_cycle_detection.rs` | Add 10 test cases |
