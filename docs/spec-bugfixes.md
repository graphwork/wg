# Specification: Agency Bootstrapping & Loop Convergence Fixes

This spec covers two bugs that degrade the out-of-box experience for wg's agency and looping systems.

---

## Bug 1: Agency Bootstrapping Gaps

**Source:** `BUG_REPORT_AGENCY_BOOTSTRAPPING.md`

### Current State

`wg agency init` (`src/commands/agency_init.rs`) already creates a default "Careful Programmer" agent (lines 23-78), seeds 4 roles and 4 motivations, and enables `auto_assign` + `auto_evaluate`. The bug report describes the *original* broken state which has since been partially fixed.

Two remaining issues exist:

#### 1a. `wg service start` warning — already implemented

`src/commands/service.rs` lines 1977-2016 already check `config.agency.auto_assign && no_agents_defined` and print a warning. **No code change needed.**

#### 1b. `wg service status` — already distinguishes "no agents defined" vs "agents all died"

`src/commands/service.rs` lines 3003-3080 already check `agency_agents_defined` and render either "No agents defined — run 'wg agency init' or 'wg agent create'" or the alive/idle/total counts. **No code change needed.**

#### 1c. Remaining gap: `wg service status` does not distinguish "agents all died" from "agents idle"

When agency agents ARE defined but `agents_alive=0`, the status shows `Agents: 0 alive, 0 idle, 0 total` — this doesn't tell the user whether agents died (failure) or were never spawned (no tasks). The status should distinguish these cases.

### Fix 1c: Add "agents died" diagnostic to service status

**File:** `src/commands/service.rs`, function `run_status()` (~line 3067)

**Change:** After the existing agents line (line 3074-3079), when `agency_agents_defined` is true but `alive_count == 0` and `coord.agents_spawned > 0` (at least one tick has spawned agents), add a note:

```rust
// After line 3079, inside the `else` branch of `if !agency_agents_defined`
if alive_count == 0 && coord.ticks > 0 && coord.agents_spawned == 0 && coord.tasks_ready > 0 {
    println!("  Note: tasks are ready but no agents have been spawned — check agent configuration");
}
```

For the JSON output (after line 3036), add a `note` field under `agents` when this condition holds.

**Test case:** Add a test `test_status_shows_note_when_tasks_ready_but_no_agents_spawned` that:
1. Creates a wg dir with an agency agent defined
2. Simulates a coordinator state with `ticks > 0`, `agents_spawned == 0`, `tasks_ready > 0`
3. Asserts the text output contains the "tasks are ready but no agents have been spawned" note

### Fix 1d: Improve rollback/error messaging in agency init

**File:** `src/commands/agency_init.rs`

**Current behavior:** If the agent creation step (step 2) fails, the roles and motivations from step 1 remain. This is fine for idempotency but could confuse users.

**Change:** Wrap the init in a way that prints a clear error message if any step fails after partial work:

```rust
// At line 109, before Ok(())
// (If we got here, everything succeeded — no change needed)
```

Actually, the current error handling already uses `.context()` on each step and the idempotent design means re-running fixes partial states. **No code change needed** — the current design is correct.

---

## Bug 2: Loop Convergence Discoverability

**Source:** `BUG_REPORT_LOOP_CONVERGENCE.md`

### Root Cause

Agents working on looping tasks don't know about `--converged` because:
1. The default prompt template (`src/service/executor.rs` lines 340-391) only mentions `wg done {{task_id}}` — no mention of `--converged`
2. The `build_task_context()` function (`src/commands/spawn.rs` lines 88-119) only pulls dependency artifacts/logs — no loop metadata
3. The quickstart text (`src/commands/quickstart.rs` lines 85-99) mentions `--converged` but only in passing; the task state transitions table omits it

### Fix 2a: Inject loop information into the agent prompt template

**File:** `src/service/executor.rs`

**Change 1 — Add `task_loop_info` to `TemplateVars`:**

At line 26 (end of `TemplateVars` struct), add:

```rust
pub struct TemplateVars {
    pub task_id: String,
    pub task_title: String,
    pub task_description: String,
    pub task_context: String,
    pub task_identity: String,
    pub working_dir: String,
    pub skills_preamble: String,
    pub model: String,
    pub task_loop_info: String,  // NEW
}
```

**Change 2 — Populate `task_loop_info` in `TemplateVars::from_task()`:**

In `from_task()` (around line 35), after the existing field assignments, compute the loop info:

```rust
let task_loop_info = if !task.loops_to.is_empty() {
    let edges: Vec<String> = task.loops_to.iter().map(|edge| {
        let max = edge.max_iterations
            .map(|m| format!(" (max {})", m))
            .unwrap_or_default();
        format!("  - loops to '{}'{}", edge.target, max)
    }).collect();
    format!(
        "This task has loop edges (iteration {}):\n{}\n\n\
         **IMPORTANT: When this loop's work is complete (converged), you MUST use:**\n\
         ```\n\
         wg done {} --converged\n\
         ```\n\
         Using plain `wg done` will cause the loop to fire again and re-open tasks.\n\
         Only use plain `wg done` if you want the next iteration to proceed.",
        task.loop_iteration,
        edges.join("\n"),
        task.id
    )
} else {
    String::new()
};
```

**Change 3 — Add `{{task_loop_info}}` to the default prompt template:**

In the default Claude executor prompt template (line 340-391), add a loop info section between the "Context from Dependencies" and "Required Workflow" sections:

```
## Context from Dependencies
{{task_context}}

{{task_loop_info}}

## Required Workflow
```

The `{{task_loop_info}}` block renders to empty string for non-looping tasks (no visual impact) and renders the full loop guidance for looping tasks.

**Change 4 — Update `apply_template()` to substitute `{{task_loop_info}}`:**

In `apply_template()` (wherever template variable substitution happens), add:

```rust
result = result.replace("{{task_loop_info}}", &vars.task_loop_info);
```

### Fix 2b: Update the default prompt's "Complete the task" section

**File:** `src/service/executor.rs`, default prompt template (line 368-371)

**Current:**
```
3. **Complete the task** when done:
   ```bash
   wg done {{task_id}}
   ```
```

**Change to:**
```
3. **Complete the task** when done:
   ```bash
   wg done {{task_id}}
   wg done {{task_id}} --converged  # Use this if task has loop edges and work is complete
   ```
```

This ensures every agent sees `--converged` as an option even without the loop-specific block.

### Fix 2c: Improve quickstart text

**File:** `src/commands/quickstart.rs`

**Change 1 — Lines 85-99, make `--converged` more prominent:**

Replace the current LOOP EDGES section with:

```
LOOP EDGES (cyclic processes)
─────────────────────────────────────────
  Some workflows repeat. A loops_to edge fires when its task completes,
  resetting a target task back to open and incrementing loop_iteration.
  Intermediate tasks in the chain are also re-opened automatically.

  wg add "Revise" --loops-to write --loop-max 3          # loop back to write
  wg add "Poll" --loops-to poll --loop-max 10 --loop-delay 5m  # self-loop with delay
  wg show <task-id>           # See loop_iteration to know which pass you're on
  wg loops                    # List all loop edges and their status

  IMPORTANT — Signaling convergence:
  When the loop's work is complete and no more iterations are needed:

    wg done <task-id> --converged

  This stops the loop. Using plain 'wg done' causes the loop to fire again.
  Only use plain 'wg done' if you want the next iteration to proceed.
```

**Change 2 — Task state transitions table (lines 73-77):**

The current transitions list:

```
  wg done <task-id>           # Mark task complete
```

Add `--converged` variant below it:

```
  wg done <task-id>           # Mark task complete (loop fires if present)
  wg done <task-id> --converged  # Complete and STOP the loop
```

**Change 3 — JSON output (lines 190-196):**

Update the `loops.convergence` value to be more emphatic:

```rust
"convergence": "IMPORTANT: Use 'wg done <task-id> --converged' to stop a loop when work is complete. Plain 'wg done' causes the loop to fire again."
```

Add to `commands.completion`:

```rust
"done_converged": "Complete task and stop loop (wg done <id> --converged)"
```

### Fix 2d: Coordinator injects loop note when spawning agents for looping tasks

**File:** `src/commands/spawn.rs`, function `build_task_context()` (lines 88-119)

**Change:** After the dependency context is built (line 117), check if the task has loop edges and append loop metadata:

```rust
fn build_task_context(graph: &workgraph::graph::WorkGraph, task: &workgraph::graph::Task) -> String {
    let mut context_parts = Vec::new();

    // ... existing dependency artifact/log code (lines 92-111) ...

    // Inject loop metadata if this task has loops_to edges
    if !task.loops_to.is_empty() {
        context_parts.push(format!(
            "Loop status: iteration {} of this task",
            task.loop_iteration
        ));
        for edge in &task.loops_to {
            let max_str = edge.max_iterations
                .map(|m| format!(", max {}", m))
                .unwrap_or_default();
            context_parts.push(format!(
                "  loops_to: '{}'{}", edge.target, max_str
            ));
        }
    }

    if context_parts.is_empty() {
        "No context from dependencies".to_string()
    } else {
        context_parts.join("\n")
    }
}
```

This puts loop metadata into `{{task_context}}` so even custom prompt templates that only use `{{task_context}}` (not `{{task_loop_info}}`) get some loop awareness.

---

## Test Cases

### Bug 1 Tests

| Test | File | Description |
|------|------|-------------|
| `test_status_shows_note_when_tasks_ready_but_no_agents_spawned` | `src/commands/service.rs` | Verify diagnostic note when agents defined but none spawned with ready tasks |

Existing tests that already cover the agency init fixes:
- `test_agency_init_creates_agent_and_config` (agency_init.rs:117) — verifies default agent creation
- `test_agency_init_idempotent` (agency_init.rs:154) — verifies re-running is safe
- `test_service_start_warns_no_agents` (service.rs:~4210) — verifies start warning
- `test_status_distinguishes_no_agents_from_dead_agents` (service.rs:4248) — verifies status distinction

### Bug 2 Tests

| Test | File | Description |
|------|------|-------------|
| `test_template_vars_include_loop_info` | `src/service/executor.rs` | Verify `task_loop_info` is populated for tasks with `loops_to` edges |
| `test_template_vars_empty_loop_info_for_non_loop_tasks` | `src/service/executor.rs` | Verify `task_loop_info` is empty string for normal tasks |
| `test_default_prompt_contains_converged` | `src/service/executor.rs` | Verify the default prompt template includes `--converged` |
| `test_build_task_context_includes_loop_metadata` | `src/commands/spawn.rs` | Verify `build_task_context()` appends loop iteration/edge info |
| `test_quickstart_converged_prominent` | `src/commands/quickstart.rs` | Verify quickstart text contains "IMPORTANT" and `--converged` in the loop section |

### Integration Test

| Test | File | Description |
|------|------|-------------|
| `test_looping_task_prompt_includes_converged_guidance` | `tests/integration_loops.rs` or similar | End-to-end: create a task with `--loops-to`, build its template vars, verify the rendered prompt contains `--converged` instructions |

---

## Summary of Changes

| File | Change | Lines |
|------|--------|-------|
| `src/service/executor.rs` | Add `task_loop_info` field to `TemplateVars` | ~26 |
| `src/service/executor.rs` | Populate `task_loop_info` in `from_task()` | ~35-60 |
| `src/service/executor.rs` | Add `{{task_loop_info}}` to default prompt template | ~350 |
| `src/service/executor.rs` | Add `--converged` to the "Complete the task" section | ~370 |
| `src/service/executor.rs` | Substitute `{{task_loop_info}}` in `apply_template()` | template substitution fn |
| `src/commands/spawn.rs` | Add loop metadata to `build_task_context()` | ~117 |
| `src/commands/quickstart.rs` | Make `--converged` prominent in LOOP EDGES section | 85-99 |
| `src/commands/quickstart.rs` | Add `--converged` to task state transitions | 75-77 |
| `src/commands/quickstart.rs` | Update JSON output for convergence | 190-196 |
| `src/commands/service.rs` | Add diagnostic note to `run_status()` | ~3079 |

**Estimated scope:** ~100-150 lines of new/modified code + ~80 lines of tests.

**No breaking changes.** All additions are backward-compatible — existing prompt templates without `{{task_loop_info}}` simply won't render the block (the substitution is additive). The quickstart changes are purely textual.
