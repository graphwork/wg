# Design: Reopening Done Tasks When New Dependencies Are Added

## Problem Statement

When a completed task gains a new upstream dependency via `wg edit --add-after` or
`wg add --after <done-task>`, the task remains in `Done` status. Its output is now
potentially stale — it completed without considering the new dependency's output.

**Concrete example:** `validate-tui-integration` was already done when
`verify-wg-tui` was added as a new dependency via `--add-after`. The coordinator
had already dispatched and completed it before the new dependency existed. The
validation is now semantically incomplete.

## Current Behavior

### Edge addition has no status checks

In `src/commands/edit.rs:74-94`, adding an `--add-after` dependency simply appends
to the task's `after` list and updates the blocker's `before` list. No status check
occurs — the operation succeeds silently regardless of whether the task is Open,
Done, Failed, or any other status.

Similarly, `src/commands/add.rs:190-225` creates new tasks with `--after` edges
pointing at existing tasks without inspecting their status.

### The only reopen mechanism is cycle iteration

The `reactivate_cycle()` function in `src/graph.rs:982-1066` is the **only** code
path that transitions tasks from `Done` → `Open`. It fires when:

1. All members of a structural cycle reach `Done`
2. Guard conditions are met (if any)
3. `max_iterations` hasn't been reached

This is triggered synchronously inside `wg done` (`src/commands/done.rs:160`).
There is no event-driven invalidation when the graph topology changes.

### No `wg reopen` command exists

The system has no manual mechanism to transition a task from `Done` back to `Open`.
The terms "reopen", "stale", and "invalidate" do not appear in the Rust source.
The existing vocabulary is "re-activate" (used for cycle iteration).

## What Other Systems Do

### Build systems: Make, Bazel, Nix

Build systems universally rebuild when inputs change. Make compares file timestamps —
if a source file is newer than its target, the target is rebuilt. Bazel and Nix go
further with content-addressed hashing: any change to an input's content invalidates
all downstream outputs.

Key principle: **the output is a function of the inputs.** If the input set changes,
the output is invalid.

Build systems also propagate invalidation transitively: if A depends on B depends on
C, and C's input changes, both B and A are rebuilt.

### Workflow orchestrators: Airflow, Prefect, Temporal

Airflow marks downstream tasks as "needs re-run" when an upstream is cleared
(re-run). The `clear` command accepts `--downstream` to cascade. Prefect and
Temporal similarly support re-running from a specific point, invalidating everything
downstream.

Key principle: **re-execution cascades downstream by default**, but the trigger is
explicit (operator clears a task).

### Task trackers: Jira, Linear, GitHub Issues

Traditional task trackers don't auto-reopen tickets when relationships change. Adding
a "blocks" link to a closed ticket doesn't reopen it. However, they also don't have
the concept of deterministic re-execution — tasks represent human work, not
reproducible computations.

### Summary of external systems

| System Type | Auto-invalidate on input change? | Transitive? |
|-------------|----------------------------------|-------------|
| Build (Make/Bazel/Nix) | Yes, always | Yes |
| Workflow (Airflow) | Explicit trigger, then yes | Yes (opt-in) |
| Task tracker (Jira) | No | N/A |

## The Cycle Analogy

Cycle iteration already implements the mechanics of reopening:

```
reactivate_cycle():
  task.status = Status::Open
  task.assigned = None
  task.started_at = None
  task.completed_at = None
  task.loop_iteration += 1
  task.log.push("Re-activated by cycle iteration...")
```

A dependency-change reopen would need the same state reset — but triggered by graph
mutation rather than by cycle completion. The `loop_iteration` field wouldn't apply
(this isn't an iteration), but the rest of the reset is identical.

## Arguments FOR Automatic Reopen

1. **Semantic correctness.** A task's output is a function of its inputs. If the
   input set grows, the output is incomplete. Keeping it `Done` is a lie — the task
   has not validated against all its dependencies.

2. **Consistency with build systems.** Users with Make/Bazel intuition expect
   downstream invalidation when inputs change. workgraph tasks, especially
   agent-executed ones, are closer to build targets than to Jira tickets.

3. **Safety.** The coordinator dispatches work based on status. A `Done` task won't
   be re-examined. If the task should have considered the new dependency, the only
   way to get correct output is re-execution.

4. **The cycle precedent.** The system already reopens tasks. This extends the same
   principle to a different trigger.

## Arguments AGAINST Automatic Reopen

1. **Cascading rework.** If task A is reopened, all tasks in A's `before` list that
   are also `Done` may need reopening too (transitive invalidation). This could
   cascade through large portions of the graph, causing significant rework.

2. **Intent ambiguity.** The user might add the dependency for documentation or
   traceability ("this task was related to that one"), not to trigger re-execution.

3. **Completed work is valid in context.** The task legitimately completed given its
   dependencies at the time. Reopening discards that work. For human tasks (not
   agent-executed), this may feel like losing progress.

4. **No idempotency guarantee.** Unlike build targets, workgraph tasks may have side
   effects or produce different results on re-execution. Automatic re-execution
   could be wasteful or harmful.

## Middle Ground Options

### Option A: Auto-reopen when new dep is unmet (Recommended)

When adding an `--add-after` edge to a `Done` task, check if the new dependency is
itself `Done`. If the new dependency is **not** Done (i.e., the edge is unmet),
automatically reopen the task.

**Rationale:** If the new dependency is already Done, its output was available when
the task ran — the task just didn't formally declare the dependency. Adding the edge
is purely structural. But if the dependency isn't Done yet, the task genuinely missed
input it should have waited for.

**Transitive propagation:** When a task reopens, check its `before` list. Any `Done`
task that depends on the reopened task should also reopen — but only if it was
completed *after* the reopened task's original completion (timestamp check prevents
unnecessary cascading for tasks that were already ordered correctly).

Implementation:

```rust
// In edit.rs, after adding the edge:
if task.status == Status::Done {
    if let Some(dep) = graph.get_task(dep_id) {
        if !dep.status.is_terminal() || dep.status == Status::Failed {
            // New dep is not done — task output is stale
            task.status = Status::Open;
            task.assigned = None;
            task.started_at = None;
            task.completed_at = None;
            task.log.push(LogEntry {
                message: format!(
                    "Reopened: new dependency '{}' is not yet complete",
                    dep_id
                ),
            });
            // Cascade downstream...
        }
    }
}
```

### Option B: Add `--reopen` flag to edit

Add an explicit `--reopen` flag to `wg edit --add-after`:

```
wg edit validate-tui --add-after verify-wg-tui --reopen
```

This gives the user full control. No surprises. The default behavior (no reopen)
preserves backward compatibility.

**Downside:** Requires the user to remember the flag. Easy to forget, leading to
silently stale tasks — the exact problem we're trying to solve.

### Option C: Warn but don't reopen

When adding a dependency to a `Done` task, print a warning:

```
⚠ Task 'validate-tui-integration' is already done.
  New dependency 'verify-wg-tui' was not available when it completed.
  Run 'wg reopen validate-tui-integration' to re-execute.
```

This requires implementing a `wg reopen` command (useful independently) and relies
on the user to act on the warning.

### Option D: "Stale" status

Introduce a new `Stale` status for tasks whose dependency set changed after
completion. The coordinator could be configured to either auto-dispatch stale tasks
or ignore them.

**Downside:** Adds status complexity. The coordinator already handles `Open` tasks.
Making `Stale` functionally equivalent to `Open` (but with a different label) adds
concept count without clear benefit.

## Recommendation

**Implement Option A (auto-reopen on unmet dep) as the default, with Option B
(--no-reopen flag) as an escape hatch.**

The combined approach:

1. **Default behavior:** When `wg edit --add-after` adds a dependency to a `Done`
   task, and the new dependency is not in a terminal-success state (`Done`), the
   task automatically reopens with a log entry explaining why.

2. **Escape hatch:** `wg edit --add-after <dep> --no-reopen` adds the edge without
   reopening, for cases where the dependency is structural/documentary.

3. **Transitive reopening:** When a task reopens, scan its `before` list. Any `Done`
   downstream task reopens too, with a log entry: `"Reopened: upstream dependency
   '<id>' was reopened"`. This cascades until it reaches tasks that aren't Done.

4. **`wg reopen` command:** Implement as a standalone command for manual use,
   independent of edge addition. Useful for cases where the user wants to re-run a
   task for any reason.

### Why this is the right default

The scenario that triggered this study — a validation task staying `Done` while its
new dependency hadn't run yet — is a **correctness bug**, not a preference. The
system told users the validation was complete when it wasn't. Requiring explicit
`--reopen` flags means the default behavior produces incorrect results.

Build systems got this right decades ago: when inputs change, outputs are invalid.
The same principle applies here. The `--no-reopen` escape hatch covers the minority
case where someone adds a dependency for documentation purposes.

### Implementation scope

1. **`wg reopen <task-id>`** — New command. Resets status to `Open`, clears
   `assigned`/`started_at`/`completed_at`. Accepts `--cascade` flag for transitive
   downstream reopening. (~50 lines in `src/commands/reopen.rs`)

2. **Auto-reopen in `edit.rs`** — After adding `--add-after` edges, check if the
   target task is `Done` and the new dep is not `Done`. If so, reopen. Respect
   `--no-reopen` flag. (~30 lines)

3. **Transitive cascade** — When reopening (either via `wg reopen --cascade` or
   auto-reopen), walk `before` edges and reopen `Done` downstream tasks. Add log
   entries for auditability. (~40 lines)

4. **Log entries** — Every reopen (automatic or manual) produces a log entry with
   the reason. This is critical for debugging cascading reopens.

### What NOT to implement

- **New `Stale` status.** Unnecessary complexity. A reopened task is just `Open`.
- **Timestamp-based cascade filtering.** Over-engineering for v1. Simple transitive
  cascade is sufficient. If it causes problems, add filtering later.
- **Auto-reopen when removing deps.** Removing a dependency doesn't invalidate the
  task's output (it had *more* information than needed, not less).
