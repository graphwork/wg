# Bug Report: Structural cycle does not re-open tasks after plain `wg done`

## Summary

A task configured as a structural cycle header (`--max-iterations 3`) completed with plain `wg done` (not `--converged`), but the cycle did not fire — predecessor tasks were not re-opened for iteration 2. The agent explicitly logged that convergence criteria were NOT met and intentionally used plain `wg done` to trigger another iteration, but nothing happened.

## Observed behavior

1. Task `benchmark-and-validate` was created with `--max-iterations 3` as the cycle header.
2. Dependency chain: `optimize-b-move` → `benchmark-and-validate` (cycle header).
3. Agent completed `benchmark-and-validate` iteration 0 with plain `wg done` (no `--converged` flag).
4. Agent log explicitly states: "Convergence criteria NOT met: 19 B/row (target ≤16), 1.6x RSS reduction (target ≥3x), SMEM still 1.6-2.3x slower than FMD (target: comparable). All tests pass and correctness verified. Using plain 'wg done' to trigger next iteration."
5. **The cycle did not fire.** `optimize-b-move` was NOT re-opened. No iteration 1 occurred.
6. The workgraph showed all tasks as "done" with 0 in-progress, 0 ready, 0 blocked.

## Expected behavior

After `wg done benchmark-and-validate` (without `--converged`), the cycle should:
1. Re-open `optimize-b-move` (the predecessor task)
2. Re-open `benchmark-and-validate` itself
3. Increment `loop_iteration` to 1
4. The coordinator should dispatch agents for the re-opened tasks

This should continue until either `--converged` is used or `max_iterations` (3) is reached.

## Contrast with loop edges (which DID work)

In an earlier session, we used `--loops-to` loop edges (the old API) and they fired correctly — tasks were re-opened, iteration counts incremented, agents spawned. The issue is specific to the newer `--max-iterations` structural cycle mechanism.

## Additional problems caused

Because the cycle didn't fire:
- The b-move optimization stopped at 19 bytes/row (target was 8-12, matching Movi)
- The agent's uncommitted changes were left floating in the worktree with no follow-up task
- We had to manually cherry-pick the partial work to master
- An entire iteration of improvement was lost

## Possible causes

1. **Structural cycles may not re-open predecessor tasks** — perhaps `--max-iterations` only sets metadata but doesn't create the actual loop edge that `--loops-to` creates?
2. **Missing loop edge** — `wg show benchmark-and-validate` showed "Cycle config (header): Max iterations: 3" but no "loops_to" edge. The `--loops-to` flag was not used because `wg add` didn't accept it (only `--max-iterations`). Maybe structural cycles and loop edges are separate mechanisms and only loop edges actually fire?
3. **The cycle header has no `loops_to` target** — the system knows it's a cycle header with max 3 iterations, but doesn't know WHICH task to re-open.

## Diagnostic data

```
$ wg show benchmark-and-validate
...
Cycle config (header):
  Max iterations: 3
...
Log:
  14:13:01 Convergence criteria NOT met... Using plain 'wg done' to trigger next iteration.
  14:13:13 Task marked as done [agent-8]
```

Note: no "Re-opened" log entry after this, unlike the old `--loops-to` system which logged "Re-opened: blocker was re-activated by loop".

## Suggested investigation

1. Clarify the relationship between `--max-iterations` (structural cycles) and `--loops-to` (loop edges). Are they the same mechanism? Different?
2. If `--max-iterations` is supposed to create a self-loop (re-open the cycle header's predecessors), verify this codepath works.
3. If `--max-iterations` requires an explicit `--loops-to` target, document this clearly. The quickstart mentions `--loops-to` with `--loop-max` but doesn't mention `--max-iterations` at all.
4. Consider whether `wg add "X" --max-iterations 3 --after Y` should implicitly create a loop edge back to Y.

## Environment

- workgraph (latest as of 2026-02-23)
- Coordinator: `wg service start --max-agents 4`
- Tasks created with `wg add ... --max-iterations 3 --after <predecessor>`
- Executor: claude, model: opus

## Workaround

Use the old `--loops-to` API instead of `--max-iterations`:
```bash
wg add "Validate" --blocked-by impl-task --loops-to impl-task --loop-max 3
```
This worked correctly in earlier sessions.
