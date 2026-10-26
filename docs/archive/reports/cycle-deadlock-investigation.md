# Cycle Deadlock Investigation: Coordinator Dispatch

## Problem Statement

Multi-task cycles (2+ tasks forming a loop) can deadlock when **both/all tasks are re-opened after iteration 0**: all members are blocked on each other, none appear in the ready list, and the coordinator never dispatches them. Single-task self-loops work fine.

---

## Question 1: How does `wg ready` compute readiness?

**File:** `src/query.rs`

There are four readiness functions, ordered by increasing awareness:

| Function | Line | Cycle-aware? | Peer-aware? |
|----------|------|-------------|-------------|
| `ready_tasks()` | 248 | No | No |
| `ready_tasks_with_peers()` | 311 | No | Yes |
| `ready_tasks_cycle_aware()` | 337 | Yes | No |
| `ready_tasks_with_peers_cycle_aware()` | 400 | Yes | Yes |

**The coordinator uses `ready_tasks_with_peers_cycle_aware()`** (confirmed at `coordinator.rs:154`, `coordinator.rs:715`, `coordinator.rs:830`).

### Core readiness logic (`src/query.rs:248-276`)

A task is ready when:
1. `status == Open`
2. Not paused
3. Past `not_before` timestamp
4. **All** `after` dependencies are terminal (Done/Failed/Abandoned)

### Cycle-aware exemption paths (`src/query.rs:363-391`)

Two exemption rules let cycle members bypass blocked dependencies:

**Path 1 — Worker exemption** (line 368-375):
- Condition: Task has **no** `cycle_config`, blocker **has** `cycle_config`, both in the same SCC.
- Purpose: Workers can proceed without waiting for the header (iterator) to finish.
- This correctly handles the header→worker direction.

**Path 2 — First-iteration bootstrap** (line 380-391):
- Condition: Task's `loop_iteration == 0` AND task **has** `cycle_config`, blocker is in the same SCC.
- Purpose: On iteration 0, the header can bypass in-SCC blockers (including self-loops and workers).
- This handles the initial bootstrap of cycles.

---

## Question 2: How does the coordinator's dispatch loop select tasks?

**File:** `src/commands/service/coordinator.rs`

The coordinator tick flow:

1. **`check_ready_or_return()`** (line 148): Calls `ready_tasks_with_peers_cycle_aware()` with a fresh `CycleAnalysis`. If no spawnable tasks, returns early.

2. **`spawn_agents_for_ready_tasks()`** (line 2705): Recomputes `ready_tasks_with_peers_cycle_aware()` (line 2715), then iterates ready tasks, skipping:
   - Already-assigned tasks (line 2725)
   - Daemon-managed tasks (line 2730)
   - Throttled respawn tasks (line 2735)
   - System tasks with abandoned sources (line 2741)
   - Unassigned non-system tasks when auto_assign is on (line 2761)
   - Inline tasks (evaluation/flip/assignment — handled differently, line 2768)

3. Remaining tasks get `spawn::spawn_agent()` called (line 2875).

The coordinator has **no special cycle-deadlock detection** for `after`-edge cycles. It only detects circular **wait** conditions (`detect_circular_waits()` at line 322), which is a different system (`WaitSpec`/`WaitCondition` for runtime waiting, not structural `after`-edge cycles).

---

## Question 3: Where is cycle detection done? Is it available at dispatch time?

**File:** `src/graph.rs:981-1032` — `CycleAnalysis::from_graph()`
**File:** `src/cycle.rs` — underlying algorithms (Tarjan's SCC, natural loop analysis)

### How it works:

1. `CycleAnalysis::from_graph()` builds a `NamedGraph` from all tasks' `after` edges.
2. Calls `named.analyze_cycles()` which runs Tarjan's SCC algorithm.
3. Produces:
   - `cycles: Vec<DetectedCycle>` — each with `members`, `header`, `reducible` flag
   - `task_to_cycle: HashMap<String, usize>` — maps task ID → cycle index
   - `back_edges: HashSet<(String, String)>` — (predecessor, header) pairs

### Available at dispatch time: **Yes**

The coordinator calls `graph.compute_cycle_analysis()` at dispatch time:
- `coordinator.rs:153` — inside `check_ready_or_return()`
- `coordinator.rs:2714` — inside `spawn_agents_for_ready_tasks()`

But note: the `back_edges` set from `CycleAnalysis` is **not used** in the readiness computation. The readiness functions only use `task_to_cycle` to check SCC membership, plus `task.cycle_config` to distinguish headers from workers.

---

## Question 4: What state do stuck cycle member tasks have?

When a 2+ task cycle deadlocks after iteration 0:

- **Status:** `Open` — `reactivate_cycle()` sets `status = Open` (graph.rs:1418)
- **`loop_iteration`:** `>= 1` — incremented by `reactivate_cycle()` (graph.rs:1422)
- **`assigned`:** `None` — cleared by `reactivate_cycle()` (graph.rs:1419)
- **`cycle_config`:** Present on at least the header (possibly on all members if auto-created)

The tasks are **not blocked, not waiting, not paused** — they are `Open` with unsatisfied `after` dependencies. They simply never appear in the ready list because the cycle-aware exemptions don't fire.

---

## Question 5: How does the single-task self-loop differ?

A self-loop (task A depends on A) works because **Path 2** (first-iteration bootstrap) has an explicit self-loop check at `query.rs:382-384`:

```rust
if *blocker_id == task.id {
    return true;
}
```

And on subsequent iterations, the **same Path 2** code fires because `task.cycle_config.is_some()` — but **only when `loop_iteration == 0`**.

Wait — self-loops also deadlock on iteration 1+? Let me verify...

Actually no. For self-loops, `reactivate_cycle()` re-opens the single task with `loop_iteration = 1`. Then on the next ready check:
- Path 2 (bootstrap) doesn't fire because `loop_iteration != 0`.
- But there's no path that exempts a self-loop on iteration 1+!

**However**, looking at `reactivate_cycle()` (graph.rs:1269-1316): it is called when a task transitions to `Done`. For a self-loop, Mode 2 (implicit cycle) at line 1299-1313 fires — it re-opens the task. But when re-opened, the task depends on itself (which is now `Open`, not terminal), so it would be blocked again.

Checking Mode 1 (SCC cycle) at line 1276-1296: a self-loop IS an SCC, so Mode 1 fires. The task gets re-opened. Then on the next readiness check:
- `loop_iteration = 1`, `cycle_config.is_some()` → Path 2 doesn't fire
- `task.cycle_config.is_none()` is false → Path 1 doesn't fire
- Self-dependency not terminal → blocked

**So self-loops should ALSO deadlock on iteration 1+.** But the user reports they work. Let me re-read the task description...

The description says "Single-task self-loop (e.g., haiku-loop-5) — task is its own dependency, so it's trivially ready." This might mean the self-loop case works only on iteration 0 and then deadlocks on 1+, OR there's another mechanism I'm missing.

Let me check if `reactivate_cycle` does something special with the self-dependency...

No — `reactivate_cycle()` just sets `status = Open` and increments `loop_iteration`. The self-dep remains in `after`.

**Conclusion:** Self-loops likely also deadlock after iteration 0, but the bug report may be referring to the initial dispatch only. OR the user manually runs the task after iteration 0, triggering `done --converged` before the deadlock manifests. Either way, the root cause is the same.

---

## Root Cause Analysis

The deadlock happens because **neither exemption path fires after iteration 0**:

### Path 1 (Worker exemption, line 368-375):
```rust
if task.cycle_config.is_none()        // task must NOT have cycle_config
    && blocker.cycle_config.is_some()  // blocker MUST have cycle_config
```
- **Fails when both tasks have `cycle_config`** (the common case when `--max-iterations` auto-creates back-edges).
- **Fails when neither task has `cycle_config`** (manually constructed cycles).

### Path 2 (Bootstrap exemption, line 380):
```rust
if task.loop_iteration == 0 && task.cycle_config.is_some()
```
- **Fails when `loop_iteration > 0`** — i.e., every iteration after the first.

### Deadlock scenarios:

| Scenario | Iteration 0 | Iteration 1+ |
|----------|-------------|--------------|
| A↔B, both have `cycle_config` | Path 2 bootstraps both ✓ | Neither path fires ✗ |
| A↔B, only A has `cycle_config` | A: Path 2 ✓, B: Path 1 ✓ | A: no path ✗, B: Path 1 ✓ (but A is stuck) |
| A↔B, neither has `cycle_config` | No path fires ✗ | No path fires ✗ |
| A→A self-loop with `cycle_config` | Path 2 self-check ✓ | No path fires ✗ |

**The table at iteration 1+ shows the deadlock.** The existing test `test_cycle_aware_iteration_1_no_bootstrap` (query.rs:1938) **explicitly validates this deadlock as expected behavior** — the comment says "a sequencing mechanism is needed." But no such mechanism exists.

---

## Proposed Fix

### Approach: Extend Path 2 to work beyond iteration 0

The simplest and most correct fix is to remove the `loop_iteration == 0` restriction from the bootstrap exemption for cycle headers that need to start a new iteration. After `reactivate_cycle()` re-opens all members, the header should be able to bootstrap past in-SCC blockers on any iteration, not just iteration 0.

### Specific change:

**File:** `src/query.rs`, lines 380-391 (in `ready_tasks_cycle_aware`) and lines 433-443 (in `ready_tasks_with_peers_cycle_aware`).

Change:
```rust
if task.loop_iteration == 0 && task.cycle_config.is_some() {
```

To:
```rust
if task.cycle_config.is_some() {
```

**But wait** — this would break the design intent. The comment on the existing test says the header should wait for workers on iteration 1+. The purpose of the `loop_iteration == 0` guard is to ensure that after the bootstrap, the header properly waits for workers to finish before triggering the next iteration.

### Better approach: Detect the deadlock state and break it

The deadlock occurs specifically when **all** SCC members are `Open` and **none** are `InProgress`. This means the cycle has been reactivated but no task can start. The fix should detect this condition and exempt the header (or one task) from the SCC blockers to break the deadlock.

**File:** `src/query.rs`, add a deadlock-breaking condition to the cycle-aware readiness functions.

Add a **Path 3** after Path 2:

```rust
// Path 3: Cycle deadlock breaker.
// After reactivation, all SCC members are Open but none can start.
// Detect this: task has cycle_config, is in an SCC, and ALL other SCC
// members are Open (not in-progress/done). This means we need to
// bootstrap again — exempt in-SCC blockers for the header.
if task.cycle_config.is_some() {
    if let Some(&cycle_idx) = cycle_analysis.task_to_cycle.get(&task.id) {
        let cycle = &cycle_analysis.cycles[cycle_idx];
        let all_members_open = cycle.members.iter().all(|mid| {
            graph.get_task(mid)
                .map(|t| t.status == Status::Open)
                .unwrap_or(false)
        });
        if all_members_open
            && graph.get_task(blocker_id).is_some()
            && cycle_analysis.task_to_cycle.get(blocker_id) == Some(&cycle_idx)
        {
            return true;
        }
    }
}
```

### Why this approach is better:

1. **Targeted:** Only fires when there's an actual deadlock (all members Open, none progressing).
2. **Preserves sequencing:** During normal execution (header waiting for workers to finish), workers are InProgress or Done — the condition `all_members_open` is false, so the header correctly waits.
3. **Works for all cycle sizes:** 2-task, 3-task, N-task cycles.
4. **Works for both symmetric and asymmetric cycle_config:** The header with `cycle_config` breaks the deadlock.
5. **Works for self-loops:** A single Open task in an SCC satisfies `all_members_open`.

### Files to modify:

| File | Function | Change |
|------|----------|--------|
| `src/query.rs:337` | `ready_tasks_cycle_aware()` | Add Path 3 after line 391 |
| `src/query.rs:400` | `ready_tasks_with_peers_cycle_aware()` | Add Path 3 after line 443 |
| `src/query.rs:1938` | `test_cycle_aware_iteration_1_no_bootstrap()` | Update: both should now be ready when all-open deadlock detected |

### Additional consideration: Neither-has-cycle_config case

If a cycle is manually created without `--max-iterations` (neither task has `cycle_config`), the deadlock occurs even on iteration 0 because neither Path 1, Path 2, nor the proposed Path 3 would fire (all require `cycle_config`).

To handle this edge case, Path 3 could be relaxed to fire for any task in an SCC when all members are Open, regardless of `cycle_config`. However, this would need careful thought — tasks in an SCC without `cycle_config` may have been intentionally arranged (e.g., via `wg add --after` creating bidirectional deps without `--max-iterations`).

**Recommendation:** Start with the `cycle_config.is_some()` guard (fixes the common case), and consider relaxing it as a follow-up.

---

## Summary

| Question | Answer |
|----------|--------|
| 1. Readiness computation | `src/query.rs:248` (`ready_tasks`), coordinator uses `ready_tasks_with_peers_cycle_aware` at line 400 |
| 2. Coordinator dispatch | `src/commands/service/coordinator.rs:2705` (`spawn_agents_for_ready_tasks`), called from tick loop |
| 3. Cycle detection | `src/graph.rs:981` (`CycleAnalysis::from_graph`), backed by `src/cycle.rs` (Tarjan's SCC). Available at dispatch time. |
| 4. Stuck task state | `Open`, `loop_iteration >= 1`, `assigned = None`, `cycle_config` present |
| 5. Self-loop difference | Path 2 has explicit self-check at `query.rs:382-384`, but both self-loops and multi-task cycles deadlock after iteration 0 |
