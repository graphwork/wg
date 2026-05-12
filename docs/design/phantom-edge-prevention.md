# Design: Phantom Edge Prevention — Validation and Error Surfacing Strategy

Task: `design-phantom-edge`  
Depends on: `research-phantom-edge` ([analysis](../research/phantom-edge-analysis.md))  
Supersedes: `dangling-dependency-resolution.md` (written before the systematic research)

## Problem Statement

A "phantom edge" is a dependency reference (`--after`) to a task ID that doesn't exist in the graph. The research identified **5 code paths** where dependencies are set, **6 concrete failure scenarios**, and a fundamental inconsistency in how the system handles phantoms.

### Root Causes

1. **`wg add --after X`** warns on stderr but creates the phantom edge anyway
2. **`wg edit --add-after X`** has zero validation — no warning, no check
3. **Bidirectional invariant broken**: the `before` backlink is silently skipped when the target doesn't exist, and never retroactively repaired when the target is later created
4. **Inconsistent handling**: `ready_tasks()` blocks on phantoms (correct), but `query::after()` silently skips them (incorrect), meaning `wg done` doesn't block on phantoms

### Why This Matters

A single phantom edge permanently blocks a task — and transitively blocks its entire downstream subgraph. The coordinator never dispatches it. `why-blocked` shows the phantom as "Open" (misleading). The only recovery is manual intervention, which requires the human to notice the problem first.

---

## Design: Defense in Depth

Five complementary mechanisms, each catching phantoms at a different stage. No single mechanism is sufficient alone — the combination provides both prevention and recovery.

### Mechanism 1: Strict Validation on `wg add --after` (Prevention)

**Decision: Strict by default, with automatic leniency for paused tasks.**

| Mode | Behavior |
|------|----------|
| `wg add "X" --after dep` | **Hard error** if `dep` doesn't exist. Typos caught immediately. |
| `wg add "X" --after dep --paused` | **Warning only** (as today). Validation deferred to `wg publish`. |
| `wg add "X" --after dep --allow-phantom` | **Warning only**. Explicit opt-in for scripts or edge cases. |

**Rationale:**
- Interactive human use (`wg add` without `--paused`) is the primary source of typo-caused phantoms. Hard failure catches these at the source.
- Batch/coordinator use (`wg add --paused`) already has a validation gate at `wg publish`. Deferring validation here is correct because the referenced tasks may not exist yet during batch construction.
- The `--allow-phantom` flag is an escape hatch for automation that deliberately creates forward references outside the paused workflow.

**Fuzzy matching on rejection:**
```
$ wg add "Deploy" --after bild-artifacts
Error: Dependency 'bild-artifacts' does not exist.
  → Did you mean 'build-artifacts'?
  Hint: Use --paused to defer validation, or --allow-phantom to allow forward references.
```

**Backward compatibility:** This is a breaking change for interactive use. Any script that does `wg add --after nonexistent` without `--paused` will now fail. Mitigation:
- The coordinator already uses `--paused` for batches, so coordinator workflows are unaffected.
- Human users get an immediate, actionable error instead of a silent block.
- `--allow-phantom` provides an escape for anyone who genuinely needs the old behavior.

**Implementation:** `src/commands/add.rs:342-362` — change the `eprintln!` warning path to `anyhow::bail!()` unless `paused` is true or a new `--allow-phantom` flag is set. Add the flag to the CLI definition.

### Mechanism 2: Batch/Publish with Deferred Validation (Already Implemented — Enforce It)

**Decision: No code changes needed. Strengthen coordinator prompt to mandate batch workflow.**

The research confirmed that `wg publish` (`src/commands/resume.rs:178-205`) already performs hard validation on all dependencies, including transitive subgraph validation. The paused/publish workflow is the correct pattern for batch creation.

**Gap to close:** The coordinator prompt recommends but does not mandate the batch workflow. The coordinator sometimes creates individual non-paused tasks, especially for single-task requests. Update the coordinator prompt to:
1. Always use `--paused --no-place` when creating 2+ tasks that reference each other
2. The quality-pass workflow already implies this; make it explicit that cross-referencing tasks MUST be paused

**No behavioral enforcement in code.** The strict validation in Mechanism 1 implicitly enforces this: if a coordinator tries `wg add "A" --after B` without `--paused` and B doesn't exist yet, it gets a hard error, forcing it to use the batch workflow.

### Mechanism 3: Graph-Level Integrity Check (Detection)

**Decision: Enhance `wg check` and add a pre-dispatch gate in the coordinator.**

#### 3a. Enhanced `wg check` output

Current `wg check` detects orphans via `check_orphans()` but the output is terse. Enhance to:

```
$ wg check
Graph integrity: 2 issues found

  Phantom edges (dependencies on non-existent tasks):
    implement-auth --after--> setup-db
      Created: 12m ago | Suggestion: 'setup-database' (edit distance: 2)
      Fix: wg edit implement-auth --remove-after setup-db
           wg edit implement-auth --add-after setup-database

    deploy-service --after--> bild-artifacts
      Created: 3m ago | Suggestion: 'build-artifacts' (edit distance: 1)
      Fix: wg edit deploy-service --remove-after bild-artifacts
           wg edit deploy-service --add-after build-artifacts
```

Key additions:
- Time since task creation (distinguishes "just created, batch in progress" from "stuck typo")
- Fuzzy match suggestions (already implemented: `check::fuzzy_match_task_id`)
- Concrete fix commands (copy-pasteable)

#### 3b. Pre-dispatch phantom check in coordinator

On each coordinator tick, before dispatching ready tasks, scan for non-paused tasks that have been blocked by phantom edges for longer than a configurable threshold (default: 2 minutes, configurable via `wg config --phantom-timeout <duration>`).

When detected:
1. Log a warning: `"Task 'X' blocked by phantom dependency 'Y' for 5m — likely a typo or missing task"`
2. Send a message to the task: `wg msg send X "⚠ Blocked by phantom dependency 'Y' (does not exist). Did you mean 'Z'?"`
3. Surface in `wg status` attention section (already partially implemented)

**Implementation:** `src/commands/service/coordinator.rs` — add a `check_phantom_edges()` call in the main loop, throttled to run at most once per minute.

#### 3c. `wg check` as CI / pre-commit hook

Document that `wg check` can be used as a CI gate:
```bash
wg check || { echo "Graph integrity check failed"; exit 1; }
```

### Mechanism 4: Consistent Phantom Handling — Soft vs Hard Edges (Consistency)

**Decision: Phantom edges are hard blocks everywhere, with explicit labeling.**

The research found a critical inconsistency:
- `ready_tasks()` treats phantoms as blocking (`.unwrap_or(false)`) — correct
- `query::after()` silently skips phantoms (`.filter_map(|id| graph.get_task(id))`) — incorrect

This means `wg done` doesn't check for phantom blockers, but automatic dispatch does. Resolve by making both paths consistent:

#### 4a. Fix `query::after()` to include phantom information

Don't filter out phantom dependencies. Instead, return enough information for callers to distinguish real from phantom blockers. Two options:

**Option A (recommended): Add a `phantom_blockers()` query.**  
Keep `after()` as-is for backward compatibility (many callers). Add:

```rust
/// Return dependency IDs that don't resolve to any task in the graph.
pub fn phantom_blockers(task: &Task, graph: &wg) -> Vec<String> {
    task.after.iter()
        .filter(|id| graph.get_task(id).is_none())
        .filter(|id| federation::parse_remote_ref(id).is_none())
        .cloned()
        .collect()
}
```

Use this in `why-blocked`, status displays, and anywhere phantom information is needed.

**Option B: Return a richer type from `after()`.**  
Change `after()` to return `Vec<BlockerInfo>` where `BlockerInfo` is an enum of `Resolved(Task)` | `Phantom(String)`. More type-safe but higher churn.

Recommendation: Option A. Lower churn, targeted fix.

#### 4b. Fix `why-blocked` to label phantoms correctly

Current behavior (`why_blocked.rs`): shows phantom blockers as "Open" because `unwrap_or(Status::Open)`.

Fix: Check for phantom explicitly:

```
$ wg why-blocked deploy-service
deploy-service is blocked by:
  ✓ setup-db (done)
  ⚠ bild-artifacts (DOES NOT EXIST — phantom dependency)
    → Did you mean 'build-artifacts'?
```

#### 4c. Fix `task_depth()` to handle phantoms

Current behavior (`graph.rs:1350`): returns 0 for unknown IDs, which can cause incorrect depth calculations in the guardrails system.

Fix: `task_depth()` should return `None` (or a sentinel) for phantom dependencies, and callers should treat this as an error rather than depth 0.

### Mechanism 5: Auto-Repair (Recovery)

**Decision: `wg fix` command for interactive repair, plus `wg check --fix` for automated repair.**

#### 5a. Interactive repair via `wg fix`

When phantom edges are detected, offer interactive repair:

```
$ wg fix
Found 2 phantom edges:

  1. implement-auth --after--> setup-db
     → Closest match: 'setup-database' (distance: 2)
     [r]eplace with 'setup-database'  [d]elete edge  [s]kip

  2. deploy-service --after--> bild-artifacts
     → Closest match: 'build-artifacts' (distance: 1)
     [r]eplace with 'build-artifacts'  [d]elete edge  [s]kip
```

This is a new subcommand. Implementation is straightforward: iterate `check_orphans()`, present options, apply edits via `modify_graph`.

#### 5b. Automated repair via `wg check --fix`

Non-interactive mode for CI/scripts:

```bash
wg check --fix          # Auto-replace if fuzzy match distance ≤ 2, auto-delete otherwise
wg check --fix --dry-run  # Show what would be changed
```

Rules:
- If fuzzy match exists with edit distance ≤ 2: auto-replace
- If no close match: auto-delete the phantom edge (with warning)
- Always log what was changed

#### 5c. Retroactive `before` backlink repair

The research found that when `wg add "A" --after "B"` is called and B doesn't exist, the `before` backlink on B is never created. If B is later created via a separate `wg add "B"`, the backlink is missing.

Fix: When a new task is created, scan all existing tasks' `after` lists for references to the new task's ID. If found, add the corresponding `before` backlink.

```rust
// In wg add, after adding the new task to the graph:
let new_id = &task.id;
for existing_task in graph.tasks_mut() {
    if existing_task.after.contains(new_id) && !existing_task.id.eq(new_id) {
        if !task.before.contains(&existing_task.id) {
            // The existing task references us; add backlink
            // (need to update the newly-added task's before list)
        }
    }
}
```

This is a targeted fix in `src/commands/add.rs` near line 428. Low risk, high impact — fixes the bidirectional invariant silently.

### Mechanism 6: `wg edit --add-after` Validation (Gap Fix)

**Not one of the original 5 mechanisms, but a critical gap identified by the research.**

`src/commands/edit.rs:111-113` adds `after` dependencies with zero validation. Apply the same validation as `wg add`:

- Hard error if the referenced task doesn't exist
- Fuzzy match suggestion on error
- `--allow-phantom` flag for explicit opt-in

This is the path of least resistance for introducing phantom edges today, and the easiest fix.

---

## Error Surfacing by Context

### CLI Output

| Command | Phantom handling |
|---------|-----------------|
| `wg add --after X` | Hard error (unless `--paused` or `--allow-phantom`) |
| `wg edit --add-after X` | Hard error (unless `--allow-phantom`) |
| `wg status` | Attention section listing phantom-blocked tasks |
| `wg check` | Full report with suggestions and fix commands |
| `wg fix` | Interactive repair wizard |
| `wg why-blocked` | Labels phantoms as "DOES NOT EXIST" with fuzzy suggestion |
| `wg viz` | Red dashed edges to phantom nodes (already implemented) |

### Service / Coordinator Logs

- Coordinator tick: warns when tasks are phantom-blocked for > threshold
- Sends `wg msg` to affected tasks for visibility in status/TUI

### TUI

- Phantom-blocked tasks shown with a distinct indicator (e.g., `⚠` prefix or red highlight)
- Phantom edges in the graph viewer rendered as dashed red (already implemented in viz)
- Attention bar at bottom showing count of phantom-blocked tasks

---

## Coordinator Batch-Creation Workflow

The standard coordinator batch workflow:

```
1. wg add "task-a" --paused --no-place --after task-b    ← phantom warning (B not yet created)
2. wg add "task-b" --paused --no-place                   ← now B exists
3. wg publish                                             ← validates ALL deps, succeeds
```

This workflow is **unchanged by this design**. The key insight: `--paused` on step 1 defers validation to `wg publish`. No `--allow-phantom` needed.

If the coordinator accidentally creates a non-paused task with a phantom dep:
- Step 1 fails with a hard error
- The coordinator gets an actionable error message
- The coordinator can retry with `--paused` or reorder its task creation

Single-task creation by the coordinator (no batch):
- `wg add "task-x" --after existing-task` — works fine, `existing-task` is validated
- No special handling needed for single tasks

---

## Edge Cases

| Scenario | Handling |
|----------|----------|
| Typo in `--after` by human | Hard error with fuzzy suggestion. Human corrects immediately. |
| Coordinator creates tasks out of order | Uses `--paused` batch workflow. Publish validates. |
| Coordinator crashes mid-batch | Paused tasks with phantom deps remain paused. Next coordinator tick detects them via phantom check. `wg fix` can repair. |
| Task deleted after being referenced | `wg edit --remove-after` on the referencing task, or `wg fix` auto-repair. The phantom check detects this. |
| Remote/federated reference (`peer:task-id`) | Exempt from local validation (as today). Validated at resolution time via federation. |
| Cyclic references (`--max-iterations`) | Back-edges are created between existing tasks. If the dep doesn't exist when the cycle is being set up, `--paused` workflow handles it. |
| `wg add` with `--after A,B,C` where B doesn't exist | Hard error listing only the missing deps. A and C are not checked further (fail fast). |
| `wg edit --add-after` on an in-progress task | Hard error if dep doesn't exist. The edit command already warns about editing in-progress tasks. |

---

## Migration / Compatibility

### Breaking Changes

1. **`wg add --after X` now fails if X doesn't exist** (unless `--paused` or `--allow-phantom`).
   - Impact: Scripts that create tasks with forward references without `--paused` will break.
   - Mitigation: Add `--allow-phantom` to affected scripts. The coordinator already uses `--paused`.

2. **`wg edit --add-after X` now fails if X doesn't exist** (unless `--allow-phantom`).
   - Impact: Same as above.

### Non-Breaking Changes

All other changes (enhanced `wg check`, `wg fix`, `why-blocked` improvements, backlink repair, coordinator phantom detection, TUI indicators) are additive and backward-compatible.

### Migration Path

1. Ship all non-breaking improvements first (Mechanisms 3, 4, 5 — detection, consistency, repair)
2. Ship `--allow-phantom` flag (no behavior change yet, just the opt-in exists)
3. Ship strict validation (Mechanisms 1, 6) as a separate commit — breaking change

This allows users to adopt `--allow-phantom` in their scripts before the default changes.

---

## Implementation Priority

| Priority | Mechanism | Effort | Impact |
|----------|-----------|--------|--------|
| **P0** | 6: `wg edit --add-after` validation | Small | Closes the widest-open gap |
| **P0** | 4b: `why-blocked` phantom labeling | Small | Fixes actively misleading output |
| **P0** | 5c: Retroactive `before` backlink repair | Small | Fixes bidirectional invariant silently |
| **P1** | 1: Strict `wg add --after` + `--allow-phantom` | Medium | Prevents phantoms at the source |
| **P1** | 3a: Enhanced `wg check` output | Medium | Better diagnostics |
| **P1** | 4a: `phantom_blockers()` query | Small | Foundation for all display improvements |
| **P2** | 3b: Coordinator pre-dispatch phantom check | Medium | Catches stuck tasks automatically |
| **P2** | 5a/5b: `wg fix` command | Medium | Recovery tooling |
| **P3** | 4c: `task_depth()` phantom handling | Small | Edge case in guardrails |
| **P3** | 2: Coordinator prompt strengthening | Small | Documentation only |

---

## Rejected Alternatives

### Forward-reference syntax (`--after ~future-task-id`)

New syntax that explicitly marks an edge as "this will exist later." Rejected because:
- Adds cognitive overhead for every user
- The `--paused` workflow already solves this without new syntax
- The tilde is arbitrary — what other prefix? `?`, `!`, `@`?

### Grace period (phantom OK for N minutes, then error)

Rejected as a primary mechanism because the threshold is arbitrary and graph construction speed varies. However, the coordinator phantom-detection threshold (Mechanism 3b) uses time as an escalation heuristic, which is the right level — it's an alert, not a hard error.

### Soft/warning-only everywhere (status quo plus better surfacing)

The existing `dangling-dependency-resolution.md` proposed this approach. Rejected because:
- Warnings are easily ignored (the original incident proves this — the warning was printed but nobody saw it)
- Detection without prevention means every phantom must be manually fixed
- The paused/publish workflow already provides the batch escape hatch, so strict validation doesn't sacrifice any real use case

### Remove phantom edges automatically after timeout

Rejected because auto-removing an edge changes the dependency graph without user intent. A phantom edge might be a forward reference that will be resolved — auto-removing it could cause a task to dispatch prematurely without its actual prerequisite.

---

## Summary

The design uses defense in depth:

1. **Prevent** — Strict validation on `wg add` and `wg edit` (hard error by default)
2. **Defer** — `--paused` tasks defer validation to `wg publish` (batch workflow)
3. **Detect** — Enhanced `wg check`, coordinator pre-dispatch scan, `wg status` attention section
4. **Diagnose** — Fuzzy matching, `why-blocked` labeling, phantom-specific queries
5. **Repair** — `wg fix` interactive/automated repair, retroactive backlink healing

The most impactful change is making `wg add --after` strict by default (Mechanism 1), because it prevents phantoms at the source rather than detecting them after the damage is done. The `--paused` batch workflow provides a clean escape hatch for coordinators that need forward references.
