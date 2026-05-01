# Design: Actor-Driven Worktree Cleanup — Status as Cleanup Intent

## 1. Survey of Current Cleanup Paths

Three cleanup mechanisms exist today, forming a two-tier model plus a manual fallback:

### 1a. Atomic sweep (happy path) — `sweep_cleanup_pending_worktrees()`

**File:** `src/commands/service/worktree.rs:725-856`

Called every coordinator tick. Scans `.wg-worktrees/agent-*/` for the `.wg-cleanup-pending` marker file. A worktree is removed iff **all** of:

1. Marker file `.wg-cleanup-pending` exists (written by agent wrapper at exit).
2. Owning agent is **not live** per `AgentEntry::is_live()` (checks `AgentStatus`, PID liveness, heartbeat freshness within 300s).
3. Owning task is in a **terminal status** (`Done | Failed | Abandoned`) or is missing from the graph.

If any gate fails, the worktree is skipped. The sweep is idempotent — if the coordinator crashes mid-sweep, the next tick retries.

**Limitation:** The terminal-status gate uses `is_terminal()` which returns `true` only for `Done | Failed | Abandoned`. It does **not** account for `Incomplete` (retryable) or `PendingValidation` (eval pending). Both are treated as non-terminal, so their worktrees are never swept by this path — they persist until the task transitions to a terminal status or the worktree ages out.

### 1b. Orphan cleanup (startup) — `cleanup_orphaned_worktrees()`

**File:** `src/commands/service/worktree.rs:594-707`

Called once at service startup. Scans `.wg-worktrees/agent-*/` for directories whose owning agent is not alive (via registry + heartbeat check). Does **not** check for the cleanup-pending marker (handles agents killed before writing it). Does **not** check task status — any dead agent's worktree is cleaned.

This is more aggressive than the sweep: it trusts that a dead agent with no registry liveness means the worktree is orphaned, regardless of task state.

### 1c. Manual GC fallback — `wg gc --worktrees`

**File:** `src/commands/worktree_gc.rs`

User-invoked, dry-run by default (`--apply` to execute, `--force` to override uncommitted-changes gate). Uses a `plan()` function that classifies each worktree into `Remove`, `Uncommitted`, or `Skip` based on a four-gate safety predicate:

1. Not the currently running agent.
2. Agent is not live (or missing from registry → conservative skip without `--force`).
3. Task is terminal or missing from graph.
4. No uncommitted changes (unless `--force`).

Gate 3 uses the same `is_terminal()` check — `Incomplete` and `PendingValidation` tasks cause a `Skip`.

### 1d. Additional mechanisms

- **`prune_stale_worktrees()`** (`worktree.rs:862-979`): Age-based pruning for worktrees of dead agents. Currently `#[allow(dead_code)]` — not wired into the coordinator.
- **`CleanupQueue` / `CleanupWorker`** (`worktree.rs:1198-1463`): Thread-safe priority queue for async cleanup jobs. Also `#[allow(dead_code)]` — infrastructure exists but is not yet active.
- **`remove_worktree()`** (`worktree.rs:134-255`): Shared removal machinery. Removes `.wg` symlink → `target/` dir → `git worktree remove --force` → `git branch -D`. Includes metrics, retry logic, and force-cleanup fallback.
- **`recover_commits()`** (`worktree.rs:442-468`): Before removing a dead agent's worktree, checks for unmerged commits and creates `recover/<branch>` branches.

### Summary: What each path checks

| Path | Marker? | Agent dead? | Task terminal? | Uncommitted gate? |
|------|---------|-------------|----------------|-------------------|
| Atomic sweep | Yes | Yes | Yes | No |
| Orphan cleanup | No | Yes | **No** | No |
| `wg gc --worktrees` | No | Yes | Yes | Yes |
| `prune_stale_worktrees` | No | Yes (age) | **No** | No |

The inconsistency in task-status checking between paths is the core problem this design addresses.

## 2. Status → Cleanup Decision Table

### Current `Status` enum (`src/graph.rs:125-136`)

```rust
pub enum Status {
    Open,
    InProgress,
    Waiting,
    Done,
    Blocked,
    Failed,
    Abandoned,
    PendingValidation,
    Incomplete,
}
```

`is_terminal()` returns `true` for `Done | Failed | Abandoned` only.

### Proposed cleanup-intent mapping

| Status | Cleanup intent | Worktree disposition | Rationale |
|--------|---------------|---------------------|-----------|
| `Open` | **preserve** | Never has a worktree (pre-dispatch). If one exists (orphan), defer to agent-liveness check. | Task hasn't started; worktree would only exist from a prior crashed attempt. |
| `InProgress` | **preserve** | Active agent is using it. | Agent is alive and working. Removing would destroy in-flight work. |
| `Waiting` | **preserve** | Agent may be parked but worktree has state. | Task is paused (e.g., waiting on human input); agent may resume. |
| `Blocked` | **preserve** | Similar to Waiting — blocked on dependencies. | Task will unblock when dependencies complete. |
| `Done` | **cleanup ok** | Work landed in main via merge-back. Worktree has no further value. | Merge-back already happened (or was skipped for zero-commit worktrees). |
| `Failed` | **cleanup ok** | Terminal failure. No retry coming unless `wg retry` resets to Open. | If user retries, a new worktree is created for the new attempt. |
| `Abandoned` | **cleanup ok** | User explicitly abandoned. No preservation value. | Strongest cleanup signal — user gave up. |
| `PendingValidation` | **preserve until eval transitions** | Eval pipeline will grade it → `Done` (cleanup) or `Incomplete` (preserve for retry). Eval is the deciding actor. | Premature cleanup would destroy the worktree before eval can inspect it. |
| `Incomplete` | **context-dependent** | If retry is queued (retry_count < max_retries): **preserve**. If retries exhausted (will transition to Failed): **cleanup ok**. | The `wg incomplete` command already handles this transition — when retries are exhausted, it sets status to `Failed`. The interesting case is the window between `Incomplete` and the next dispatch. |

### Edge cases

| Scenario | Status | Decision | Reason |
|----------|--------|----------|--------|
| Incomplete, retries remaining, `incomplete_retry_delay` active | `Incomplete` | **preserve** | Next attempt benefits from warm worktree. |
| Incomplete, retries exhausted → auto-transitions to `Failed` | `Failed` | **cleanup ok** | The `wg incomplete` command (`src/commands/incomplete.rs:75-84`) already transitions to `Failed` when `retry_count >= effective_max`. |
| PendingValidation, eval fails (task → Incomplete) | `Incomplete` | **preserve** | Eval said "not done yet" — next agent retry inherits the prior diff. |
| PendingValidation, eval passes (task → Done) | `Done` | **cleanup ok** | Work is accepted; merge-back already happened at done-time. |
| Cycle member, iteration complete (task → Done, cycle resets → Open) | `Open` (after reset) | **cleanup ok** (worktree belongs to prior iteration's agent) | The cycle reset creates a new Open task; the old agent's worktree is from a completed iteration. |
| User runs `wg retry` on Failed task | `Open` (after retry) | N/A (old worktree already eligible for cleanup when Failed; new attempt gets fresh worktree) | `wg retry` clears `assigned`, `session_id`, `checkpoint` — no worktree reuse path exists today. |
| `--preserve` flag on `wg done` | `Done` | **preserve** (override) | User explicitly wants to inspect the worktree post-completion. |

### Proposed helper: `cleanup_eligible()`

Rather than overloading `is_terminal()`, add a dedicated method:

```rust
impl Status {
    pub fn cleanup_eligible(&self) -> bool {
        matches!(self, Status::Done | Status::Failed | Status::Abandoned)
    }
}
```

This is identical to `is_terminal()` today, but the naming communicates intent. The key behavioral change is in the **coordinator sweep logic** (Section 4), which adds `Incomplete`-aware handling.

## 3. Retry-State Inheritance: Warm Reuse (Option A)

### Decision: Option A — Warm reuse

When a task transitions from `Incomplete` → `Open` (via retry) and is re-dispatched, the same worktree directory is reused if it still exists.

### Rationale

| Factor | Option A: Warm reuse | Option B: Snapshot inheritance |
|--------|---------------------|-------------------------------|
| **Disk cost** | Zero (reuse in-place) | 44MB checkout + rsync of `target/` (~33GB) |
| **Time cost** | Zero (worktree already exists) | 10-30s for rsync of large `target/` |
| **Build cache** | Full cache warm — incremental builds are near-instant | Warm cache after rsync, but rsync itself is expensive |
| **Coupling** | Ties retry to same on-disk path; agent must handle stale state | Clean slate with known-good cache; no stale state risk |
| **Complexity** | Low: just don't delete the worktree | Medium: rsync logic, temp dir management, cleanup of source worktree |
| **Git state** | Prior agent's uncommitted changes visible in `git status` | Clean checkout (rsync copies only `target/`, not git state) |

**Why A wins:** The primary cost driver is cargo `target/` (33GB). With warm reuse, the retry agent inherits the full build cache at zero cost. The stale-state concern is manageable: the retry agent sees any prior uncommitted changes in `git status` and can choose to `git reset --hard HEAD` for a clean start or build on the prior diff. This is strictly more information than Option B provides, at lower cost.

**Stale-state mitigation:** The agent wrapper can prepend a `git status` summary to the retry agent's context, showing what the prior attempt left behind. The agent guide already instructs agents to check `git status` before working.

### Implementation sketch

When the coordinator dispatches a retry for task T:

1. Check if `.wg-worktrees/agent-<old-id>/` exists and has a branch for task T.
2. If yes: pass `WG_WORKTREE_PATH=<existing-path>` and `WG_BRANCH=<existing-branch>` to the new agent. Skip `create_worktree()`.
3. If no (worktree was already cleaned up): create a fresh worktree as today.

The "if yes" path requires the coordinator to look up the prior agent's worktree path from the registry before the old agent entry is archived. This means the sweep must **not** clean up `Incomplete` task worktrees — which is exactly what the status-driven model ensures.

### Fallback

If the worktree was cleaned prematurely (e.g., operator ran `wg gc --worktrees --apply --force`), the retry simply gets a cold worktree. No crash, no data loss — just a slower first build.

## 4. Coordinator Sweep Logic Redesign

### Current behavior (single predicate)

```
for each .wg-worktrees/agent-*/:
    if marker AND agent_dead AND task.is_terminal():
        remove_worktree()
```

### Proposed behavior (status-aware state machine)

```
for each .wg-worktrees/agent-*/:
    if agent_is_live():
        SKIP — agent is actively using the worktree

    task = lookup_task(agent.task_id)

    match task.status:
        Done | Failed | Abandoned:
            if has_preserve_flag(task):
                SKIP — user requested preservation
            else if marker_present:
                REMOVE — happy-path atomic cleanup
            else:
                REMOVE — dead agent, terminal task, no marker (orphan path)

        Incomplete:
            if retry_queued(task):     # retry_count < max_retries
                PRESERVE — next retry inherits this worktree
            else:
                # retries exhausted; task will transition to Failed on next
                # incomplete call, but may be in limbo. Be conservative.
                PRESERVE — wait for explicit Failed transition

        PendingValidation:
            PRESERVE — eval pipeline hasn't decided yet

        InProgress:
            PRESERVE — should not happen (agent is dead but task is
                        in-progress → triage will unclaim it)

        Open | Waiting | Blocked:
            if marker_present:
                # Unusual: agent wrote marker but task isn't terminal.
                # This can happen if the agent exited abnormally after
                # writing the marker but before status transition.
                PRESERVE — wait for triage to resolve task status
            else:
                PRESERVE — no cleanup signal
```

### Pseudocode

```rust
fn sweep_worktrees(dir: &Path) -> Result<usize> {
    let registry = AgentRegistry::load(dir)?;
    let graph = load_graph(&dir.join("graph.jsonl"))?;
    let mut removed = 0;

    for worktree in list_agent_worktrees(dir)? {
        let agent_id = worktree.agent_id();

        // Gate 1: never touch a live agent's worktree
        if registry.is_live(&agent_id) {
            continue;
        }

        let task_id = registry.task_id_for(&agent_id)
            .or_else(|| infer_task_id_from_branch(&worktree));
        let task = task_id.and_then(|id| graph.get_task(&id));
        let status = task.map(|t| t.status);
        let has_preserve = task.map(|t| t.preserve_worktree).unwrap_or(false);
        let has_marker = worktree.has_cleanup_marker();

        let should_remove = match status {
            // Terminal statuses: cleanup unless preserved
            Some(Status::Done | Status::Failed | Status::Abandoned) => {
                !has_preserve
            }
            // Incomplete: preserve for retry
            Some(Status::Incomplete) => false,
            // PendingValidation: preserve for eval
            Some(Status::PendingValidation) => false,
            // Active/blocked statuses: preserve
            Some(Status::InProgress | Status::Waiting | Status::Blocked) => false,
            // Open: only if marker present AND task was reset from terminal
            // (handles cycle resets where prior iteration's worktree lingers)
            Some(Status::Open) => {
                has_marker && !has_preserve
            }
            // Task missing from graph (already GC'd): safe to remove
            None => true,
        };

        if should_remove {
            remove_worktree(&worktree)?;
            removed += 1;
        }
    }

    Ok(removed)
}
```

### Cycle handling (the autopoietic-v2 case)

When a cycle iteration completes:
1. All cycle members transition to `Done`.
2. Cycle header resets members to `Open` and increments `loop_iteration`.
3. The prior iteration's agent is dead, its worktree has a marker, and its task is now `Open`.
4. Under the proposed logic: `Open` + marker → **remove**. This is correct — the cycle reset means a new agent will be dispatched with a fresh worktree.

If the cycle member is `Incomplete` (eval said "try again"), the worktree is preserved for the retry within the same iteration.

### Consistency with `wg gc --worktrees`

The `plan()` function in `worktree_gc.rs` should adopt the same status-aware logic. Replace the current `!status.is_terminal()` gate with explicit matching:

```rust
// In plan():
if let Some(status) = task_status {
    match status {
        Status::Done | Status::Failed | Status::Abandoned => {
            // Falls through to Remove (pending other gates)
        }
        Status::Incomplete | Status::PendingValidation => {
            decisions.push(Decision::Skip { reason: "retryable/pending-eval" });
            continue;
        }
        _ => {
            decisions.push(Decision::Skip { reason: "non-terminal" });
            continue;
        }
    }
}
```

## 5. Override Surface: `--preserve` Flag

### Where it's added

- **`wg done <id> --preserve`**: Keeps the worktree even though status is Done.
- **`wg fail <id> --preserve`**: Keeps the worktree for post-mortem inspection even though retries are exhausted.

`wg abandon` does **not** get `--preserve` — abandonment is an unambiguous cleanup signal.

### How it's persisted

Add a `preserve_worktree` field to the `Task` struct in `src/graph.rs`:

```rust
pub struct Task {
    // ... existing fields ...

    /// When true, the task's worktree is preserved even after terminal status.
    /// Set by `wg done --preserve` or `wg fail --preserve`.
    /// Cleared by `wg gc --worktrees --force` or manual `wg abandon`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub preserve_worktree: bool,
}
```

This is a task-level field (not a metadata sidecar or sweep-skip marker file) because:
1. It participates in the same graph serialization as all other task state.
2. It's visible via `wg show` — operators can see which tasks have preserved worktrees.
3. It survives coordinator restarts (persisted in `graph.jsonl`).
4. `wg gc --worktrees --force` can override it by ignoring the field.

### Clearing the preserve flag

- `wg gc --worktrees --apply --force` removes preserved worktrees (the `--force` flag overrides).
- `wg abandon <id>` transitions to Abandoned, which is cleanup-eligible regardless of `preserve_worktree`.
- Manual `wg task update <id> --no-preserve` (or editing graph.jsonl) clears it.

### Alternative considered: marker file

A `.wg-preserve` marker file inside the worktree was considered but rejected:
- It's invisible to `wg show` and `wg list`.
- It can be orphaned if the worktree is partially cleaned.
- It requires filesystem checks during sweep (already done for `.wg-cleanup-pending`, but adding more markers increases complexity).

## 6. Migration Plan

### Phase 1: Add `preserve_worktree` field (non-breaking)

1. Add `preserve_worktree: bool` to `Task` in `src/graph.rs` with `#[serde(default)]`.
2. Add `--preserve` flag to `wg done` and `wg fail` CLI in `src/cli.rs`.
3. Wire the flag into `src/commands/done.rs` and `src/commands/fail.rs`.

**Impact on in-flight tasks:** None. The field defaults to `false`, which is the current behavior. Existing `graph.jsonl` files deserialize correctly (missing field → `false`).

### Phase 2: Status-aware sweep (behavioral change)

1. Refactor `sweep_cleanup_pending_worktrees()` to use the status-aware decision table from Section 4.
2. Replace the `!task.status.is_terminal()` check with explicit match on all `Status` variants.
3. Add `Incomplete` preservation: when task is `Incomplete` and agent is dead, skip sweep (preserve for retry).
4. Add `PendingValidation` preservation: when task is `PendingValidation`, skip sweep (preserve for eval).
5. Add `preserve_worktree` check: when task has `preserve_worktree=true`, skip sweep.

**Impact on in-flight tasks:** Strictly less aggressive than today. Tasks that were previously swept (terminal + dead agent) continue to be swept. Tasks that were previously orphaned (Incomplete without marker) remain unswept. The only new behavior is that `Incomplete` tasks with markers are now explicitly preserved rather than swept.

### Phase 3: Warm retry reuse

1. When coordinator dispatches a retry for task T, check for an existing worktree from the prior attempt.
2. If found, reuse it (pass `WG_WORKTREE_PATH` and `WG_BRANCH` to the new agent).
3. If not found, create fresh worktree as today.

**Impact on in-flight tasks:** None for non-retry tasks. Retry tasks get a warm worktree instead of cold — strictly better.

### Phase 4: Update `wg gc --worktrees`

1. Update `plan()` in `worktree_gc.rs` to use the same status-aware logic.
2. Add `PendingValidation` and `Incomplete` to the `Skip` classification.
3. Respect `preserve_worktree` in the `plan()` function (Skip unless `--force`).

**Impact:** Dry-run output changes (some worktrees that were previously classified as `Remove` are now `Skip`). No behavioral change until `--apply`.

### Rollout safety

- Phases 1–2 can ship in a single commit. Phase 1 is pure additive (new field + CLI flag). Phase 2 changes sweep behavior but only makes it less aggressive.
- Phase 3 is independent and can ship separately.
- Phase 4 aligns `wg gc` with the coordinator's logic — should ship with or after Phase 2.

## 7. Implementation Breakdown — Follow-Up Tasks

### Task 1: `add-preserve-worktree-field`

Add `preserve_worktree: bool` to `Task` struct, wire `--preserve` into `wg done` and `wg fail`.

**Files:** `src/graph.rs`, `src/cli.rs`, `src/commands/done.rs`, `src/commands/fail.rs`
**Verify:** `cargo test` passes; `wg done test-task --preserve` sets `preserve_worktree=true` in graph.

### Task 2: `status-aware-worktree-sweep`

Refactor `sweep_cleanup_pending_worktrees()` to use explicit `match` on all `Status` variants. Preserve `Incomplete` and `PendingValidation` worktrees. Respect `preserve_worktree` flag.

**Files:** `src/commands/service/worktree.rs`
**Verify:** New integration tests: `Incomplete` task worktree preserved; `PendingValidation` worktree preserved; `Done --preserve` worktree preserved; `Done` (no preserve) worktree removed.

### Task 3: `warm-retry-worktree-reuse`

When dispatching a retry for an `Incomplete` → `Open` task, check for and reuse the prior agent's worktree. Pass existing `WG_WORKTREE_PATH` and `WG_BRANCH` to the new agent.

**Files:** `src/commands/spawn/execution.rs`, `src/commands/service/coordinator.rs`
**Verify:** Retry of incomplete task reuses existing worktree; build cache is warm; `cargo build` is incremental.

### Task 4: `align-gc-worktrees-with-sweep`

Update `plan()` in `worktree_gc.rs` to use the same status-aware classification as the coordinator sweep. Skip `Incomplete`, `PendingValidation`, and `preserve_worktree` tasks.

**Files:** `src/commands/worktree_gc.rs`
**Verify:** `wg gc --worktrees` shows `[skip]` for `Incomplete` and `PendingValidation` tasks; `--force` overrides `preserve_worktree`.

### Task 5: `cleanup-orphan-startup-align`

Align `cleanup_orphaned_worktrees()` (service startup) with the status-aware model. Currently it ignores task status — add task status checking so `Incomplete` worktrees are preserved even across service restarts.

**Files:** `src/commands/service/worktree.rs`
**Verify:** Service restart does not clean up worktree for `Incomplete` task; does clean up worktree for `Failed` task with dead agent.

### Dependency graph

```
add-preserve-worktree-field
    ├──> status-aware-worktree-sweep
    │       └──> cleanup-orphan-startup-align
    └──> align-gc-worktrees-with-sweep

(depends on status-aware-worktree-sweep)
warm-retry-worktree-reuse
```

Tasks 1→2→5 form a pipeline. Task 4 can run in parallel with 2. Task 3 depends on Task 2 (the sweep must preserve `Incomplete` worktrees before retry can reuse them).
