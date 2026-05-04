# Design: Clean Resolution for Dangling/Phantom Dependency References

## Problem

When `wg add --after some-nonexistent-id` is used, it creates a dependency on a task that doesn't exist. The current behavior:

1. **`add.rs:174`** prints `Warning: blocker 'X' does not exist in the graph` to stderr
2. The dependency is still created
3. **`query.rs:283` (`is_blocker_satisfied`)** treats missing blockers as unsatisfied → task never becomes ready
4. **`check.rs:139` (`check_orphans`)** detects these as `OrphanRef` entries

The task is silently stuck forever. A recent real-world incident: a typo (`implement-stream-capture` vs `implement-phase-1-stream`) caused a task chain to be stuck for 7+ minutes before anyone noticed.

### The Tension

We can't reject non-existent dependencies at creation time because:
- **Cycles** require creating tasks in stages — task A depends on task B, but B doesn't exist yet when A is created
- **Forward references** are a valid and common pattern in burst graph construction

## How Other Systems Handle This

| System | Approach |
|--------|----------|
| **Airflow** | DAG validation at parse time; all task IDs must exist before the DAG is loaded. No forward refs allowed. |
| **Prefect** | Tasks are Python objects; references are resolved by object identity, not string ID. Dangling refs are impossible at the language level. |
| **Temporal** | Workflows are code; dependencies are function calls. No string-based ID references. |
| **Make** | Missing prerequisites cause an immediate error unless the target has a rule. `make --warn-undefined-variables` for softer checks. |
| **Bazel/Buck** | Strict validation — all targets must exist. Build fails immediately on unknown reference. |
| **Terraform** | Plan phase validates all references before apply. Unknown refs are hard errors. |
| **Nix** | Lazy evaluation — references are resolved when needed. Missing refs error at evaluation time, not definition time. |

**Key insight:** Most systems either (a) require all refs to exist at definition time, or (b) use language-level references that can't dangle. workgraph's string-based IDs with support for forward refs is unusual and requires a hybrid approach.

## Recommended Approach: Layered Detection + Surfacing

Rather than one silver bullet, use multiple complementary mechanisms at different points in the workflow. This matches how the problem manifests: typos need fast feedback, while forward refs need patient tolerance.

### Layer 1: Enhanced Warning at `wg add` Time (immediate feedback)

**Current:** Warning printed to stderr, easily missed.

**Proposed:** Warning printed to stderr AND a structured annotation stored on the dependency edge.

- When `wg add --after X` is used and X doesn't exist, tag the dependency as `unresolved` with a timestamp in the task's metadata.
- When X is later created (via another `wg add`), the annotation auto-clears (the dependency is now satisfied by a real node).
- This is zero-cost for the common forward-ref case: by the time the graph is ready, all annotations have cleared.

**UX:**
```
$ wg add "Implement auth" --after setup-db
Warning: blocker 'setup-db' does not exist yet (will be treated as unresolved until created)
Created task 'implement-auth' [blocked by: setup-db (unresolved)]
```

### Layer 2: `wg status` Surfaces Unresolved Dependencies (passive monitoring)

**Current:** `wg status` shows task counts but nothing about dangling deps.

**Proposed:** Add an "Attention" section to `wg status` output when there are problems:

```
⚠ Attention:
  2 tasks have unresolved dependencies (blockers that don't exist):
    implement-auth → setup-db (unresolved for 8m)
    deploy-service → build-artifacts (unresolved for 3m)
  Run 'wg check' for details.
```

**Implementation:** Call `check_orphans()` from the status command and display results. Filter to only `after` relation orphans (those are the ones that block tasks). Include time-since-creation from the task's `created` field to help distinguish "just created, waiting for forward ref" from "stuck due to typo".

### Layer 3: `wg check` as the Authoritative Lint Tool (on-demand deep check)

**Current:** `wg check` already detects orphan refs via `check_orphans()`. The `commands/check.rs` displays them.

**Proposed:** Enhance the output to be more actionable:

```
$ wg check
Orphan references (dependencies on non-existent tasks):
  implement-auth --after--> setup-db       (task created 12m ago)
  deploy-service --after--> build-artifacts (task created 3m ago)

Suggestions:
  - Did you mean 'setup-database'? (fuzzy match found)
  - Run 'wg add "setup-db" ...' to create the missing task
  - Run 'wg edit implement-auth --remove-after setup-db' to remove the dependency
```

**Key addition:** Fuzzy matching against existing task IDs. Use edit-distance (Levenshtein) to suggest "did you mean X?" when a close match exists. This catches the exact typo scenario from the incident.

### Layer 4: Viz Integration (visual surfacing)

**Current:** `wg viz` shows the graph but doesn't distinguish dangling edges.

**Proposed:** Render dangling dependency edges distinctly:
- **DOT output:** Use `style=dashed, color=red` for edges pointing to non-existent nodes. Add a phantom node with `shape=none, fontcolor=red, label="⚠ setup-db (missing)"`.
- **TUI viz viewer:** Show dangling edges as red/dashed lines (if the renderer supports it), or add a red label on the dependent task.

### Layer 5: Coordinator Awareness (automated escalation)

**Current:** The coordinator (`coordinate.rs:87`) treats missing blockers as unresolved, which is correct — tasks with phantom deps never get dispatched. But the coordinator doesn't report this.

**Proposed:** During each poll cycle, the coordinator should check for tasks that have been blocked by unresolved deps for longer than a configurable threshold (default: 5 minutes). When found:

1. Log a warning to the coordinator log
2. Send a message to the task via `wg msg send` so the user/orchestrator sees it in `wg status` or `wg msg read`
3. Optionally (future): trigger a triage action

This catches the "silently stuck" scenario that burned 7+ minutes.

## What NOT to Do

### Explicit Forward-Ref Syntax (`--after ~future-task-id`)

Rejected because:
- Adds cognitive overhead — users must predict whether a task exists
- The tilde syntax is arbitrary and adds a new concept to learn
- The auto-resolution approach (layer 1) handles this transparently without new syntax

### Grace Period (unresolved OK for N minutes, then error)

Rejected as a standalone solution because:
- Arbitrary time threshold — 5 min? 10 min? Depends on graph construction speed
- But the coordinator awareness (layer 5) uses time as a *heuristic for escalation*, which is the right level — it's an alert, not a hard error

### Hard Rejection of Non-Existent Dependencies

Rejected because it breaks the cycle/forward-ref pattern that is fundamental to workgraph.

## Implementation Sketch

### Files to Modify

| File | Change |
|------|--------|
| `src/commands/add.rs` | Enhanced warning message (cosmetic, line ~174) |
| `src/commands/status.rs` | Add orphan-ref section using `check_orphans()` |
| `src/commands/check.rs` | Add fuzzy-match suggestions, time-since-creation |
| `src/check.rs` | Add `created_at` to `OrphanRef` struct; add fuzzy-match helper |
| `src/commands/viz.rs` or `src/tui/viz_viewer/render.rs` | Dangling edge rendering |
| `src/commands/coordinate.rs` | Unresolved-dep escalation logging + messaging |

### New Dependencies

- `strsim` crate (or similar) for Levenshtein distance / fuzzy matching. Alternatively, implement a simple edit-distance function inline (~20 lines).

### Prioritized Implementation Order

1. **`wg status` integration** — highest impact, lowest effort. Just call `check_orphans()` and display results. Immediately catches the "silently stuck" problem.
2. **`wg check` fuzzy matching** — catches typos with actionable suggestions. Medium effort.
3. **Enhanced `wg add` warning** — improves the immediate feedback. Low effort.
4. **Coordinator escalation** — catches the "7 minutes stuck" scenario automatically. Medium effort.
5. **Viz integration** — nice to have, lower priority. Medium effort.

### Estimated Scope

- Items 1-3: ~200 lines of new code across 4-5 files
- Item 4: ~50 lines in `coordinate.rs`
- Item 5: ~80 lines in viz/render code

## Summary

The recommended approach is **layered detection + surfacing** across 5 layers: enhanced add-time warnings, status integration, check/lint with fuzzy matching, viz rendering, and coordinator escalation. No new syntax or hard rejections — just better visibility at every level. The highest-priority item is `wg status` integration, which would have caught the original incident within one poll cycle.
