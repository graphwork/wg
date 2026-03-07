# SESE: Task Self-Decomposition via Single-Entry Single-Exit Regions

*Design document for runtime task decomposition in workgraph.*

---

## Motivation

When a large task is assigned to a single agent, the agent often fails partway through (context exhaustion, token limits, transient errors). The entire task must be retried, losing all progress. Example: `toctou-phase2-command` needed to convert 9 files. One agent attempted all 9, failed 5 times, wasting compute on files already converted.

The solution: let agents **decompose tasks at runtime** into independently-restartable subtasks. Each subtask is atomic — one file, one commit, one restart on failure. The decomposition preserves the original task's position in the graph (its inbound and outbound edges stay intact).

This pattern is called **SESE** (Single Entry, Single Exit) because the decomposed subgraph has exactly one entry point (receives the original task's inbound dependencies) and one exit point (feeds the original task's outbound dependents).

---

## Core Concept

Every task is a potential SESE region. When an agent determines that a task is too large for inline execution, it **splits** the task into three layers:

```
                    ┌─────────────────────────────────────────┐
  predecessors ───► │ ENTRY (plan phase)                      │
                    │   - Agent plans the decomposition       │
                    │   - Creates subtasks                    │
                    │   - Waits for subtasks to complete      │
                    └─────────┬───────────────────────────────┘
                              │
                    ┌─────────▼───────────────────────────────┐
                    │ INTERNAL SUBTASKS                        │
                    │   ┌──────────┐  ┌──────────┐            │
                    │   │subtask-1 │  │subtask-2 │  ...       │
                    │   └──────────┘  └──────────┘            │
                    └─────────┬───────────────────────────────┘
                              │
                    ┌─────────▼───────────────────────────────┐
                    │ EXIT (integration phase)                 │
                    │   - Verifies all subtasks completed      │
                    │   - Runs integration tests               │
                    │   - Commits any merge/glue work          │
                    └─────────┬───────────────────────────────┘
                              │
                              ▼  successors
```

**Key invariant:** External edges are preserved. Predecessors still point at the entry. Successors still wait on the exit. The SESE region is an implementation detail invisible to the rest of the graph (unless you expand it).

---

## Mechanism 1: `wg split <task-id>`

**What it does:** Transforms a single task into an entry/exit pair.

### Semantics

```bash
wg split my-task
```

1. The original task `my-task` becomes the **entry node**. Its metadata is preserved:
   - `after` edges (inbound dependencies) stay on the entry
   - Status stays `in-progress` (the agent is working in the entry)
   - Description, tags, skills, etc. are retained

2. A new **exit node** `my-task.exit` is created:
   - `before` edges (outbound dependents) are **moved** from the entry to the exit
   - Status: `open` (will become ready when all subtasks complete)
   - Gets a copy of the original task's `verify` criteria (final verification happens at exit)
   - Description: auto-generated noting this is the integration phase

3. The entry node gets new metadata:
   - `sese_role: "entry"` — marks this as a SESE entry
   - `sese_exit: "my-task.exit"` — points to its paired exit
   - `before` edges are replaced: entry now has `before: ["my-task.exit"]`

4. The exit node gets:
   - `sese_role: "exit"` — marks this as a SESE exit
   - `sese_entry: "my-task"` — points back to its paired entry
   - `after: ["my-task"]` — initially depends only on entry (subtasks added later)

### Data model changes

New fields on `Task` in `graph.rs`:

```rust
/// SESE region metadata. Present only on entry/exit nodes.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub sese: Option<SeseMetadata>,
```

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SeseMetadata {
    /// Role of this node in the SESE region
    pub role: SeseRole,
    /// ID of the paired entry or exit node
    pub peer: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SeseRole {
    Entry,
    Exit,
}
```

### CLI output

```
$ wg split my-task
Split 'my-task' into SESE region:
  Entry: my-task (you are here — plan subtasks)
  Exit:  my-task.exit (will activate when subtasks complete)

Add subtasks with:
  wg add "subtask title" --after my-task --before my-task.exit
```

### Validation rules

- Task must be `in-progress` (only the assigned agent should split)
- Task must not already be split (no `sese` field)
- Agent environment variable `$WG_TASK_ID` must match (prevent splitting others' tasks)

---

## Mechanism 2: `wg add --before <exit-id>`

**What it does:** Adds a subtask that is wired between the entry and exit of a SESE region.

### Semantics

```bash
wg add "Convert fail.rs" --after my-task --before my-task.exit \
  --verify "cargo test test_fail passes" \
  -d "Convert src/commands/fail.rs from load_graph+save_graph to mutate_workgraph"
```

This creates a subtask with:
- `after: ["my-task"]` — depends on the entry node
- `before: ["my-task.exit"]` — the exit node depends on it

The exit node's `after` list is automatically updated to include the new subtask.

### `--before` flag specification

New flag on `wg add`:

```
--before <task-id>    This task must complete before the specified task
                      (adds the new task to the target's `after` list)
```

This is the inverse of `--after`. When `--before X` is specified:
1. The new task gets `before: ["X"]` in its own record
2. Task X gets the new task added to its `after` list

**Note:** `--before` is general-purpose — it works for any task, not just SESE exits. It's the missing dual of `--after` that happens to be essential for SESE.

### Subtask properties

Subtasks created between entry and exit are normal tasks. They get:
- Dispatched by the coordinator to other agents
- Their own commits, logs, artifacts
- Independent retry on failure (only the failed subtask restarts)
- No special `sese` metadata (they're just regular tasks wired into the region)

---

## Mechanism 3: `wg wait` (existing, extended)

**What it does:** The entry-node agent parks itself until all subtasks complete, then the exit activates.

### Flow

After the entry agent creates all subtasks:

```bash
# Agent creates subtasks...
wg add "Convert fail.rs" --after my-task --before my-task.exit
wg add "Convert done.rs" --after my-task --before my-task.exit
wg add "Convert resume.rs" --after my-task --before my-task.exit
# ...

# Agent marks entry as done (planning complete)
wg done my-task
```

When the entry node completes (`wg done`), the coordinator's normal readiness logic handles the rest:
- Each subtask has `after: ["my-task"]` — the entry being done makes them ready
- The exit has `after: ["my-task", "convert-fail-rs", "convert-done-rs", ...]` — it becomes ready only when ALL subtasks (and the entry) are done
- The coordinator dispatches the exit to an agent for integration

**No special `wg wait` needed.** The existing dependency mechanism handles SESE natively. The entry agent's job is to plan and create subtasks, then mark itself done. The exit agent's job is to integrate and verify.

### Alternative: Entry agent stays alive

In some cases, the entry agent may want to stay alive and handle the exit phase too. For this, use the existing `wg wait`:

```bash
# After creating subtasks:
wg wait my-task --until "task:convert-fail-rs=done,task:convert-done-rs=done,..."
  --checkpoint "All 9 conversions dispatched. Resume to verify integration."
```

When all conditions are met, the coordinator resumes the entry agent, which can then do exit-phase work inline. This avoids spawning a separate exit agent but is more fragile (the entry agent's context may be stale).

**Recommendation:** Prefer the `wg done` + separate exit agent approach. It's more robust and cheaper (exit agent starts fresh with clear context).

---

## Mechanism 4: Prompt Guidance

### When to decompose

Agents should decompose when:

| Signal | Threshold | Action |
|--------|-----------|--------|
| Files to modify | > 3 | Split: one subtask per file |
| Independent steps | > 3 | Split: one subtask per step |
| Estimated tokens | > 100k output | Split: break into phases |
| Prior failures | task has retry_count > 0 | Consider splitting on retry |

### When NOT to decompose

- Task is small and well-scoped (< 3 files, < 3 steps)
- All changes are tightly coupled (splitting would create merge conflicts)
- Decomposition overhead exceeds the work itself

### Agent prompt addition (for wg skill)

```markdown
## Task Decomposition (SESE)

If your task is too large to complete reliably in one session, decompose it:

1. **Split** your task: `wg split $WG_TASK_ID`
2. **Create subtasks** between entry and exit:
   ```bash
   wg add "Subtask 1" --after $WG_TASK_ID --before $WG_TASK_ID.exit
   wg add "Subtask 2" --after $WG_TASK_ID --before $WG_TASK_ID.exit
   ```
3. **Complete the entry** (planning is done): `wg done $WG_TASK_ID`
4. The coordinator dispatches subtasks to other agents.
5. When all subtasks finish, the exit node activates for integration.

**Rule of thumb:** If > 3 files or > 3 independent steps, decompose.
Each subtask should be: one file, one commit, independently restartable.
```

---

## Worked Example: TOCTOU Phase 2

### Before (monolithic)

```
predecessors → [toctou-phase2-command] → successors
```

One agent, 9 files, fails 5 times. Each failure loses all work.

### After (SESE decomposition)

The agent receives `toctou-phase2-command`, sees 9 files to convert, and splits:

```bash
wg split toctou-phase2-command
wg add "Convert fail.rs"       --after toctou-phase2-command --before toctou-phase2-command.exit --verify "cargo test -- fail passes"
wg add "Convert done.rs"       --after toctou-phase2-command --before toctou-phase2-command.exit --verify "cargo test -- done passes"
wg add "Convert resume.rs"     --after toctou-phase2-command --before toctou-phase2-command.exit --verify "cargo test -- resume passes"
wg add "Convert edit.rs"       --after toctou-phase2-command --before toctou-phase2-command.exit --verify "cargo test -- edit passes"
wg add "Convert add.rs"        --after toctou-phase2-command --before toctou-phase2-command.exit --verify "cargo test -- add passes"
wg add "Convert link.rs"       --after toctou-phase2-command --before toctou-phase2-command.exit --verify "cargo test -- link passes"
wg add "Convert reclaim.rs"    --after toctou-phase2-command --before toctou-phase2-command.exit --verify "cargo test -- reclaim passes"
wg add "Convert reschedule.rs" --after toctou-phase2-command --before toctou-phase2-command.exit --verify "cargo test -- reschedule passes"
wg add "Convert assign.rs"     --after toctou-phase2-command --before toctou-phase2-command.exit --verify "cargo test -- assign passes"
wg done toctou-phase2-command
```

Resulting graph:

```
predecessors
    │
    ▼
[toctou-phase2-command]  (entry, done — planning complete)
    │
    ├──► [convert-fail-rs]        ──┐
    ├──► [convert-done-rs]        ──┤
    ├──► [convert-resume-rs]      ──┤
    ├──► [convert-edit-rs]        ──┤
    ├──► [convert-add-rs]         ──┤
    ├──► [convert-link-rs]        ──┤
    ├──► [convert-reclaim-rs]     ──┤
    ├──► [convert-reschedule-rs]  ──┤
    └──► [convert-assign-rs]      ──┤
                                    ▼
                   [toctou-phase2-command.exit]  (integration)
                                    │
                                    ▼
                               successors
```

**Failure impact:** If `convert-edit-rs` fails, only that one subtask retries. The other 8 keep their progress. Total loss: one file's work, not nine.

---

## Edge Cases

### Nesting (SESE within SESE)

A subtask can itself be split. For example, if `convert-fail-rs` turns out to involve multiple independent refactoring steps:

```bash
# Agent working on convert-fail-rs realizes it's complex
wg split convert-fail-rs
wg add "Extract error types" --after convert-fail-rs --before convert-fail-rs.exit
wg add "Rewrite handler"     --after convert-fail-rs --before convert-fail-rs.exit
wg done convert-fail-rs
```

This creates a nested SESE region:

```
[toctou-phase2-command] (entry)
    ├──► [convert-fail-rs] (entry, nested)
    │        ├──► [extract-error-types]   ──┐
    │        └──► [rewrite-handler]        ──┤
    │                                       ▼
    │        [convert-fail-rs.exit]  ──────────┐
    ├──► [convert-done-rs]                     ──┤
    ├──► ...                                   ──┤
                                                ▼
                        [toctou-phase2-command.exit]
```

**No special handling needed.** Nesting works naturally because SESE regions are just dependency subgraphs. The exit node of the inner region is a normal task that the outer exit depends on (transitively, through the original subtask's position).

**Depth limit:** The existing `max_task_depth` guardrail (default 8) prevents runaway nesting. Each level of nesting adds depth.

### Cycles (SESE inside a cycle)

A task that's part of a cycle can be split. The SESE region exists within a single iteration of the cycle.

```
[review-code] ──► [apply-fixes] ──► [run-tests] ──┐
      ▲                                            │
      └────────────────────────────────────────────┘
                    (cycle, max_iterations=3)
```

If `apply-fixes` is split on iteration 2:

```
[review-code] ──► [apply-fixes] (entry)
                       ├──► [fix-module-a]     ──┐
                       └──► [fix-module-b]     ──┤
                                                ▼
                  [apply-fixes.exit] ──► [run-tests] ──► ...
```

**How it works:**
- The entry/exit pair replaces the original task's position in the cycle
- The cycle header's `cycle_config` is unaffected (it's on `review-code`)
- On the next iteration, the cycle resets all member tasks — including subtasks created during the SESE split
- **Important:** The coordinator must include SESE subtasks when resetting cycle members. Implementation: when resetting a cycle, find all tasks whose `after` edges trace back to a SESE entry within the cycle, and reset those too.

**Alternative (simpler):** On cycle reset, the entry node is reset to `open` and its `sese` metadata is cleared. The subtasks become orphans (their blocker no longer exists in its split form). The coordinator can either:
1. **Abandon orphaned subtasks** — they have a dangling `after` reference, so they'll never become ready. Clean them up during cycle reset.
2. **Preserve subtasks** — if they completed successfully, keep their done status. The re-split on the next iteration can skip already-done files.

**Recommendation:** Option 1 (abandon on cycle reset) is simpler and matches the cycle's semantics of "start fresh each iteration." If subtask results should persist, the agent should commit them before the cycle resets.

### Failure and Retry of Subtasks

Individual subtask failure doesn't affect the SESE region:

- **Subtask fails:** Coordinator retries the subtask (per `max_retries`). Other subtasks continue.
- **All retries exhausted:** Subtask stays `failed`. Exit node never becomes ready (it depends on the failed subtask).
- **Recovery:** User can `wg retry <subtask>` manually. Or `wg fail <exit>` to fail the whole region, which propagates to downstream tasks.

### Entry agent fails before creating subtasks

If the entry agent crashes before completing planning:

- The entry task is `failed` (or `in-progress` with a dead agent)
- No subtasks exist yet, no exit node exists
- Normal retry: the next agent gets the original task, can choose to split again or try inline

**If the entry agent partially created subtasks before failing:**

- Some subtasks exist, wired to a non-existent or incomplete exit
- The retried entry agent can check for existing subtasks (`wg list` filtered by `--after my-task`) and either:
  - Adopt them and create remaining subtasks
  - Abandon them and start fresh

### Empty SESE (split but no subtasks created)

If an agent splits a task but then marks the entry done without creating any subtasks:

- Exit node has `after: ["my-task"]` — entry is done, so exit becomes ready immediately
- Exit agent runs, finds nothing to integrate, marks done
- This is a valid (if pointless) degenerate case

---

## Visualization (`wg viz` / TUI)

### Collapsed view (default)

SESE regions should be collapsible in visualization. By default, show the region as a single node with a visual indicator:

```
predecessors
    │
    ▼
[toctou-phase2-command ⊞]  ← ⊞ indicates expandable SESE
    │                          (3/9 subtasks done)
    ▼
successors
```

### Expanded view

When expanded (click in TUI, or `--expand-sese` flag):

```
predecessors
    │
    ▼
┌─ toctou-phase2-command ─────────────────────┐
│  [entry] ✓                                   │
│    ├──► [convert-fail-rs] ✓                  │
│    ├──► [convert-done-rs] ✓                  │
│    ├──► [convert-resume-rs] ✓                │
│    ├──► [convert-edit-rs] ⟳ (in-progress)    │
│    ├──► [convert-add-rs] ○ (open)            │
│    ├──► [convert-link-rs] ○                  │
│    ├──► [convert-reclaim-rs] ○               │
│    ├──► [convert-reschedule-rs] ○            │
│    └──► [convert-assign-rs] ○               │
│  [exit] ○ (waiting for subtasks)             │
└──────────────────────────────────────────────┘
    │
    ▼
successors
```

### Implementation

In `viz/mod.rs`, detect SESE regions by finding tasks with `sese.role == Entry`. Collect the entry, its paired exit, and all tasks between them (tasks with `after` containing the entry and `before` containing the exit). Render as a cluster/group.

The TUI (`tui/viz_viewer/`) can track collapsed/expanded state per SESE region. Toggle with Enter key on a collapsed SESE node.

---

## Implementation Plan

### Phase 1: Data model (graph.rs)

**File:** `src/graph.rs`

1. Add `SeseMetadata` and `SeseRole` structs (as specified above)
2. Add `sese: Option<SeseMetadata>` field to `Task`
3. Add serde support with `skip_serializing_if`
4. Update `TaskRaw` deserialization to handle `sese` field

**Estimated scope:** ~30 lines added to graph.rs

### Phase 2: `wg split` command

**New file:** `src/commands/split.rs`

1. Implement `split::run(dir, task_id)`:
   - Load graph, validate task (in-progress, not already split)
   - Create exit node with moved `before` edges
   - Update entry node: set `sese`, replace `before` with exit ref
   - Save graph
2. Register in `src/cli.rs` as a new `Split` subcommand
3. Wire in `src/main.rs` dispatch

**Estimated scope:** ~100 lines in split.rs, ~15 lines in cli.rs/main.rs

### Phase 3: `--before` flag on `wg add`

**File:** `src/commands/add.rs`, `src/cli.rs`

1. Add `--before` CLI flag to the `Add` variant in `cli.rs`
2. In `add::run()`, handle `before` parameter:
   - Set `before` on new task
   - Update target task's `after` list to include new task
   - Validate: target task exists (or warn)

**Estimated scope:** ~20 lines in add.rs, ~5 lines in cli.rs

### Phase 4: Cycle integration

**File:** `src/graph.rs` (cycle reset logic)

1. In `evaluate_cycle_iteration` / cycle reset code, handle SESE:
   - When resetting a cycle member that has `sese.role == Entry`, also reset/abandon subtasks
   - Clear `sese` metadata on reset so the next iteration starts clean

**Estimated scope:** ~30 lines

### Phase 5: Visualization

**File:** `src/commands/viz/mod.rs`, `src/tui/viz_viewer/`

1. Detect SESE regions in viz rendering
2. Add collapsed/expanded rendering for SESE clusters
3. Add status summary (N/M subtasks done) for collapsed view

**Estimated scope:** ~80 lines in viz, ~40 lines in TUI

### Phase 6: Agent prompt updates

**File:** `~/.claude/skills/wg/SKILL.md` (or equivalent skill template)

1. Add SESE decomposition guidance to agent prompts
2. Include the "when to decompose" decision table
3. Add `wg split` and `wg add --before` to command reference

**Estimated scope:** ~30 lines of prompt text

---

## Summary

| Mechanism | Command | New/Modified | Complexity |
|-----------|---------|--------------|------------|
| Split | `wg split <id>` | New command | Low |
| Wire subtasks | `wg add --before <exit>` | New flag on existing | Low |
| Entry completion | `wg done <entry>` | Existing, no changes | None |
| Exit activation | Coordinator readiness check | Existing, no changes | None |
| Cycle reset | Cycle iteration logic | Small modification | Low |
| Visualization | `wg viz` / TUI | Moderate addition | Medium |

The design is deliberately minimal. SESE regions are not a new graph primitive — they're a **pattern** built on existing primitives (tasks, `after`/`before` edges, status transitions). The only new concepts are:
1. `SeseMetadata` on Task (marks entry/exit pairs)
2. `wg split` command (creates the pair)
3. `--before` flag on `wg add` (the missing dual of `--after`)

Everything else — readiness, dispatch, retry, cycles — works through existing mechanisms.
