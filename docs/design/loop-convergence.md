# Loop Convergence: Breaking Autopoietic Loops Early

## Problem

Autopoietic loops (spec → implement → validate → refine → loop back to spec) run
their full `max_iterations` even after the work has converged. The refine agent may
determine on iteration 1 that everything is complete, but the loop mechanism fires
unconditionally on `wg done`, re-opening the entire chain for iterations 2 and 3.

### Observed Failure (agency-federation loop, max 3)

| Iteration | What happened |
|-----------|---------------|
| 1 | Implemented federation, 44 tests pass, refine says "CONVERGED" |
| 2 | Loop fires, agents re-run on already-working code, tests still pass, refine says "CONVERGED" again |
| 3 | Loop fires again, agents have nothing to do, stumble over each other, fail |

The refine agent *knew* the work was done but had no way to tell the loop system.

### Root Cause

`evaluate_loop_edges()` (graph.rs:566) fires whenever the source task transitions
to Done, checking only:
1. Guard conditions (Always, TaskStatus, IterationLessThan)
2. `target.loop_iteration < max_iterations`

There is no "break" or "converged" signal.

## Design

### Chosen Approach: `wg done <task> --converged`

Add a `--converged` flag to `wg done`. When set, the task gets a `"converged"` tag
and `evaluate_loop_edges()` skips all loop edge firing for that source task.

### Why This Approach

| Option | Pros | Cons | Verdict |
|--------|------|------|---------|
| **(a) `wg break-loop <edge-id>`** | Explicit, per-edge control | New command, agent must know edge IDs, race conditions | Over-engineered |
| **(b) `wg done --converged`** | Simple, auditable, uses existing tags field, one flag | Binary (converged or not) | **Chosen** |
| **(c) `ScoreAbove` guard** | Ties to evaluation system | Eval is async/optional, score may not exist when loop fires | Coupling risk |
| **(d) Abandon to prevent re-fire** | Works today | Semantic abuse — abandoned ≠ converged | Confusing |
| **(e) Output parsing for CONVERGED marker** | No code changes | Fragile, unstructured, requires output capture | Unreliable |

### Implementation

#### 1. CLI: Add `--converged` flag to `wg done`

```
wg done <ID> [--converged]
```

When `--converged` is passed:
- Add `"converged"` to `task.tags` (the field already exists on Task)
- Log entry includes convergence signal: "Task marked as done (converged)"
- Then proceed to `evaluate_loop_edges()` as normal (the check happens inside)

#### 2. Loop Evaluation: Check convergence tag

In `evaluate_loop_edges()` (graph.rs:566), add an early return at the top:

```rust
pub fn evaluate_loop_edges(graph: &mut wg, source_id: &str) -> Vec<String> {
    // Check if the source task signaled convergence — skip all loop firing
    if let Some(task) = graph.get_task(source_id) {
        if task.tags.contains(&"converged".to_string()) {
            return vec![];
        }
    }

    // ... existing logic unchanged ...
}
```

This is the **only** change to the loop evaluation logic. Everything else is
additive — the existing guards, max_iterations, and intermediate re-opening all
work exactly as before.

#### 3. Intermediate Tasks on Convergence

When the source signals convergence, `evaluate_loop_edges()` returns empty,
so **no tasks are re-opened**. The entire chain stays Done. This is correct:
if the refine step says "converged," the intermediate tasks should not re-run.

#### 4. Interaction with Evaluation

Convergence and evaluation are orthogonal:
- Convergence = "the loop's iterative refinement has reached a stable state"
- Evaluation = "how well did the agent perform on this task"

An agent can signal convergence while still receiving a low evaluation score.
The evaluation system runs after `wg done` anyway, so it observes the final
state regardless of whether the loop continues.

If a future use case needs score-based convergence, it can be added as a new
`LoopGuard::ScoreAbove(f64)` variant without changing this mechanism. The two
are composable: `--converged` is an agent-driven break, score guards would be
a system-driven break.

### What Agents Need to Know

Agents completing a task that is the source of a loop edge should:

1. **Check if the work has converged** — are all tests passing? Is the
   implementation complete? Did the previous iteration already solve everything?
2. If converged: `wg done <task-id> --converged`
3. If not converged: `wg done <task-id>` (loop fires as normal)

This information should be included in the task prompt for loop source tasks.
The coordinator can detect that a task has `loops_to` edges and add a note
to the prompt about the `--converged` flag.

### Edge Cases

- **`--converged` on a task with no loop edges**: Harmless. The tag is added,
  `evaluate_loop_edges()` returns early, but it would have returned empty anyway.
  No behavioral change.

- **`--converged` on intermediate tasks**: No effect on the loop. Only the
  source task's convergence tag matters (it's checked by source_id).

- **Multiple loop edges on one source**: `--converged` stops ALL of them.
  If per-edge convergence is ever needed, use `wg break-loop <edge-id>` (future).

- **Retry after convergence**: `wg retry` should clear the `"converged"` tag
  along with status/assigned/etc, so the loop can fire again if needed.

### Files to Change

| File | Change |
|------|--------|
| `src/main.rs` | Add `--converged` flag to done subcommand CLI definition |
| `src/commands/done.rs` | Accept `converged` param, add tag before loop eval |
| `src/graph.rs` | Add convergence check at top of `evaluate_loop_edges()` |
| `src/commands/retry.rs` | Clear `"converged"` tag on retry |
| `tests/integration_loops.rs` | Add tests for early convergence |

### Tests

1. **Loop converges early**: A→B→C with loop from C→A (max 3). C completes
   with `--converged` on iteration 1. Verify A, B, C all stay Done, no
   re-opening. Verify loop_iteration stays at 1.

2. **Loop runs to max**: Same setup but C completes without `--converged`.
   Verify normal loop firing. (Already tested, but verify no regression.)

3. **Convergence on retry**: C converges, then is retried. Verify the
   `"converged"` tag is cleared and the loop can fire again.

4. **Convergence with no loop edges**: Task completes with `--converged`.
   Verify it just gets the tag, no errors.

5. **Multiple loop edges, one convergence**: Source has loops to A and B.
   Source completes with `--converged`. Neither A nor B re-activates.
