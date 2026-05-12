# Current Validation Mechanisms in wg

Research deliverable for task `research-current-validation`.

---

## 1. Evaluation System

### Auto-Evaluation Pipeline

**File:** `src/commands/service/coordinator.rs:347-490`

The coordinator's `build_auto_evaluate_tasks()` function automatically creates `evaluate-{task-id}` tasks for every non-internal task in the graph. These evaluation tasks are blocked by their source task and become ready when the source completes (done or failed).

**Key behaviors:**
- Tasks tagged `evaluation`, `assignment`, or `evolution` are excluded to prevent infinite regress (line 374-381)
- Human agent tasks are excluded — their work quality isn't a reflection of role+motivation prompts (line 356-362)
- Abandoned tasks are excluded (line 390)
- Failed tasks DO get evaluated — there's useful signal in failure patterns (line 457-487, unblocks eval tasks whose source failed)

**Effectiveness: STRONG** — Comprehensive coverage. Every non-meta task gets evaluated automatically. The exclusion rules prevent infinite loops.

### Evaluation Execution

**File:** `src/commands/evaluate.rs:36-324`

The `wg evaluate run <task-id>` command spawns a Claude agent with a structured evaluator prompt and expects JSON output with:
- `score` (0.0-1.0)
- `dimensions` map (correctness, completeness, efficiency, style_adherence)
- `notes` (free text)

**Validation gates in evaluate.rs:**
- Task must be `Done` or `Failed` to be evaluated (line 54-63)
- Score validated to [0.0, 1.0] range for manual `evaluate record` (line 338)
- JSON extraction handles noisy LLM output with progressive fallbacks: raw → strip fences → find first `{...}` (line 563-596)

**Effectiveness: MODERATE** — The evaluation *happens* but has no enforcement power. A low score is recorded but does not block downstream tasks, trigger retries, or prevent the agent from being reassigned. Scores are purely informational for the evolution system.

### Evaluator Prompt & Rubric

**File:** `src/agency/prompt.rs:183-302`

The evaluator prompt includes:
- Task definition (title, description, skills, verification criteria)
- Agent identity (role, motivation, acceptable/unacceptable trade-offs)
- Task artifacts list
- Task log entries
- Timing information
- Scoring rubric: correctness, completeness, efficiency, style_adherence (each 0.0-1.0)

The prompt uses the task's `verify` field as "Verification Criteria" if present (line 207-209).

**Effectiveness: MODERATE** — The rubric is well-defined but the evaluator only sees artifact *paths*, not artifact *content*. It cannot read files, run tests, or verify correctness empirically. Evaluation is entirely based on log entries and metadata.

### Inline Evaluation Spawning

**File:** `src/commands/service/coordinator.rs:492-628`

The coordinator spawns evaluation tasks via a lightweight inline path (`spawn_eval_inline`) instead of the full agent machinery. This forks `wg evaluate run <source-task>` directly, eliminating run.sh wrapper overhead.

**Effectiveness: STRONG** — Engineering optimization that makes evaluations cheaper and faster, increasing the likelihood they actually complete.

---

## 2. Cycle Guards & Convergence Checks

### LoopGuard Types

**File:** `src/graph.rs:21` (enum definition), `src/graph.rs:904-931` (evaluation)

```rust
pub enum LoopGuard {
    Always,                          // Always re-iterate (trivial)
    TaskStatus { task, status },     // Continue if task has given status
    IterationLessThan(u32),          // Continue while iteration < N
}
```

`evaluate_guard()` at line 904 checks:
- `None` or `Always` → always true
- `IterationLessThan(n)` → always true (the iteration limit check at line 1022-1025 handles the actual bound)
- `TaskStatus { task, status }` → checks if the referenced task matches the specified status

**Effectiveness: MODERATE** — The guard types are limited. There's no guard based on artifact content, test results, or evaluation scores. The `TaskStatus` guard requires creating a separate validator task, which is a good pattern but entirely manual.

### Cycle Iteration Logic

**File:** `src/graph.rs:933-1066`

`evaluate_cycle_iteration()` is called from `done.rs:160` when a task completes. It:
1. Determines if the task is in a cycle (SCC-detected or implicit)
2. Checks ALL cycle members are `Done`
3. Checks convergence tag (only when no external guard is set) — line 996-1007
4. Checks `max_iterations` — line 1010-1016
5. Evaluates guard condition — line 1019-1025
6. Re-opens all cycle members for next iteration — line 1041-1063

**Effectiveness: STRONG** — This is robust. Max-iterations provides a hard upper bound. The convergence tag system allows agents to signal "I'm done" but can be overridden by external guards. The self-convergence bypass prevention (done.rs:67-113) is a particularly good design.

### Self-Convergence Prevention

**File:** `src/commands/done.rs:64-113`

When `--converged` is passed, the system checks whether the task (or its cycle) has a non-trivial guard. If so, `--converged` is ignored with a warning. This prevents agents from bypassing external validation by self-declaring convergence.

The check covers:
1. The task's own `cycle_config.guard` (line 72-77)
2. Any guard set on cycle header tasks containing this task (line 82-97)

**Effectiveness: STRONG** — Critical safety mechanism. Prevents agents from escaping their validation loops. Well-tested (6 dedicated tests, lines 466-617).

---

## 3. Done/Fail Workflow

### Blocker Validation on Done

**File:** `src/commands/done.rs:32-62`

Before marking a task done, the system checks for unresolved blockers (tasks in `after` that aren't completed). Back-edge blockers within the same cycle are exempted (the cycle iterator shouldn't block workers).

**Effectiveness: STRONG** — Prevents premature completion of tasks with unresolved dependencies. The cycle-aware exemption is well-designed.

### Task Status Transitions

**File:** `src/commands/done.rs:15-212`

Transitions validated:
- Already-done tasks return Ok (idempotent) — line 20-23
- Nonexistent tasks fail — line 18
- Status is set to `Done`, `completed_at` timestamp recorded — line 120-121
- Log entry created with actor attribution — line 127-137

**What's NOT validated:**
- No check that the task's deliverables were actually produced
- No check that artifacts exist on disk
- No check that tests pass
- No check that the `verify` field criteria are met
- Any agent can mark any task as done (no authorization check)

**Effectiveness: WEAK** — The mechanical transition is correct, but there's no quality gate. An agent can `wg done` a task without producing any output.

### Output Capture

**File:** `src/commands/done.rs:196-209`

After marking done, `capture_task_output()` saves git diff, artifacts, and log for evaluation. This is best-effort — failures are logged as warnings, not errors.

**Effectiveness: MODERATE** — Good for provenance but non-blocking. If capture fails, the task is still marked done.

### Agent Archival

**File:** `src/commands/done.rs:183-194`

Agent conversation (prompt + output) is archived for provenance. Also best-effort.

**Effectiveness: MODERATE** — Good audit trail but no validation gate.

---

## 4. Coordinator Checks

### Dead Agent Detection & Triage

**File:** `src/commands/service/triage.rs:1-200`

The coordinator detects dead agents (process exited) and runs LLM-based triage when `auto_triage` is enabled:

1. Detects dead process via PID check (line 25-36)
2. Reads agent output log (truncated to configurable max bytes, default 50KB) — line 223-267
3. Builds triage prompt describing the task and log — line 270-307
4. LLM returns verdict: `done`, `continue`, or `restart` — line 210-220
5. Applies verdict: marks done, adds continuation context, or resets to open — line 392+

**Effectiveness: MODERATE** — Smart recovery mechanism, but the LLM triage has no ground truth. It assesses based on log output, which may be misleading. The "done" verdict in particular risks marking incomplete work as complete.

### Ready Task Computation

**File:** `src/commands/service/coordinator.rs:70-95`

`check_ready_or_return()` uses cycle-aware ready task computation. Only tasks with all blockers resolved become ready.

**Effectiveness: STRONG** — Correct dependency resolution prevents premature task starts.

### Agent Slot Management

**File:** `src/commands/service/coordinator.rs:30-66`

`cleanup_and_count_alive()` enforces `max_agents` limit, preventing runaway parallelism.

**Effectiveness: STRONG** — Simple, reliable resource constraint.

### Auto-Assignment Regress Prevention

**File:** `src/commands/service/coordinator.rs:139-147`

Tasks tagged `assignment`, `evaluation`, or `evolution` are skipped for auto-assignment to prevent infinite `assign-assign-assign-...` chains.

**Effectiveness: STRONG** — Critical guard against infinite regress.

---

## 5. Graph-Level Structural Checks

### `wg check` Command

**File:** `src/check.rs:1-195`, `src/commands/check.rs:1-241`

`check_all()` runs four structural checks:

| Check | Function | Location | Severity | Description |
|-------|----------|----------|----------|-------------|
| Cycle detection | `check_cycles()` | check.rs:39-59 | Warning | DFS-based cycle finder |
| Orphan references | `check_orphans()` | check.rs:139-175 | **Error** | References to non-existent nodes (after, before, requires) |
| Stale assignments | `check_stale_assignments()` | check.rs:93-108 | Warning | Open tasks with agent assigned (dead agent?) |
| Stuck blocked | `check_stuck_blocked()` | check.rs:112-136 | Warning | Blocked tasks whose dependencies are all terminal |

Only orphan references make `ok = false`. Cycles, stale assignments, and stuck blocked are warnings.

**Effectiveness: MODERATE** — Good detection but `wg check` is not run automatically. It's a manual diagnostic tool. The coordinator doesn't run it before spawning agents. Tasks can be created with orphan references (add.rs only warns, doesn't block — line 112-117).

### Structural Cycle Analysis (Tarjan's SCC)

**File:** `src/commands/check.rs:29-120`

Beyond the basic DFS cycle detection, `check.rs` also computes Tarjan's SCC and classifies cycles as reducible vs. irreducible. Irreducible cycles (multiple entry points) get an extra warning.

**Effectiveness: MODERATE** — Good diagnostic but advisory only.

### Edge Addition Cycle Check

**File:** `src/cycle.rs:522-576`

`check_edge_addition()` uses BFS to check whether adding an edge would create a cycle. This is used proactively when edges are added, not just at check time.

**Effectiveness: STRONG** — Proactive cycle prevention is better than post-hoc detection. The incremental cycle detector (line 588+) maintains topological order for efficient repeated checks.

---

## 6. Plan Validator (Trace Functions)

**File:** `src/plan_validator.rs:1-578`

`validate_plan()` validates LLM-generated task plans against structural constraints:

| Constraint | Field | Description |
|-----------|-------|-------------|
| Task count bounds | `min_tasks`, `max_tasks` | Plan size limits |
| Required skills | `required_skills` | Skills that must appear across all tasks |
| Required phases | `required_phases` | Tags that must appear (e.g., "test", "review") |
| Forbidden patterns | `forbidden_patterns` | Tag combinations that must NOT appear |
| Cycle control | `allow_cycles`, `max_total_iterations` | Whether cycles are permitted and iteration limits |
| Depth limit | `max_depth` | Maximum dependency chain depth |

**Effectiveness: STRONG** — Well-designed constraint system for generative functions. Comprehensive test coverage (18 tests). However, this only applies to trace functions, not to manually created task graphs.

### Function Validation

**File:** `src/function.rs:644-678`

`validate_function()` validates trace function definitions:
- No duplicate template IDs
- All `after` references point to valid template IDs
- All `loops_to` targets point to valid template IDs

**Effectiveness: STRONG** — Prevents creation of malformed trace functions.

---

## 7. Input Validation (add command)

**File:** `src/commands/add.rs:44-200`

When creating tasks via `wg add`:
- Title cannot be empty (line 67-69)
- Visibility must be `internal`, `public`, or `peer` (line 72-78)
- Context scope validated against allowed values (line 81-83)
- Duplicate task IDs rejected (line 97-99)
- Self-blocking rejected (line 107-108)
- Non-existent blocker references produce a **warning** (not error) (line 112-117)
- `--cycle-guard` and `--cycle-delay` require `--max-iterations` (line 147-149)
- Guard expression syntax validated (line 9-42)
- Delay format validated (line 133-136)

**Effectiveness: MODERATE** — Good basic validation but soft on graph integrity. Non-existent blocker refs only warn, allowing orphan references to be created. This is intentional (tasks may be added in any order) but leaves the graph in a potentially invalid state.

---

## 8. Evolution Self-Mutation Safety

**File:** `src/commands/evolve.rs:306-370`

When the evolver proposes operations targeting its own role or motivation, those operations are deferred to a verified wg task requiring human approval instead of being applied immediately.

**Effectiveness: STRONG** — Critical safety mechanism preventing the evolver from modifying its own selection criteria. Well-implemented with lineage tracking.

---

## Summary: Validation Mechanism Inventory

| Mechanism | Location | Trigger | Enforcement | Rating |
|-----------|----------|---------|-------------|--------|
| Auto-evaluation creation | coordinator.rs:347 | Coordinator tick | Automatic | Strong |
| LLM evaluation scoring | evaluate.rs:36 | Task completion | Non-blocking | Moderate |
| Evaluator rubric | prompt.rs:183 | Each evaluation | Advisory | Moderate |
| Cycle max-iterations | graph.rs:1010 | Task done | Hard stop | Strong |
| Loop guard evaluation | graph.rs:904 | Cycle iteration | Blocking | Moderate |
| Self-convergence prevention | done.rs:64 | `--converged` flag | Blocking | Strong |
| Blocker validation | done.rs:32 | `wg done` | Blocking | Strong |
| Dead agent triage | triage.rs:310 | Agent death | Automatic | Moderate |
| Structural checks | check.rs:178 | `wg check` manual | Non-blocking | Moderate |
| Orphan detection | check.rs:139 | `wg check` manual | Error (but manual) | Moderate |
| Stale assignment detection | check.rs:93 | `wg check` manual | Warning | Weak |
| Stuck blocked detection | check.rs:112 | `wg check` manual | Warning | Weak |
| Edge cycle prevention | cycle.rs:522 | Edge addition | Blocking | Strong |
| Plan validation | plan_validator.rs:55 | Function apply | Blocking | Strong |
| Function validation | function.rs:644 | Function create | Blocking | Strong |
| Task input validation | add.rs:44 | `wg add` | Partial (warns) | Moderate |
| Auto-assign regress guard | coordinator.rs:139 | Coordinator tick | Blocking | Strong |
| Evolution self-mutation | evolve.rs:306 | `wg evolve` | Deferred/blocking | Strong |

## Key Gaps Identified

1. **No quality gate on `wg done`**: Any agent can mark any task done without producing deliverables, passing tests, or meeting verification criteria. The `verify` field exists but is only used as evaluator context — it's not enforced.

2. **Evaluation scores are informational only**: Low evaluation scores don't trigger retries, block downstream tasks, or flag issues. The evolution system uses them over time, but there's no immediate feedback loop.

3. **`wg check` is manual**: Structural checks aren't run automatically by the coordinator. Orphan references, stale assignments, and stuck blocked tasks can persist indefinitely.

4. **Evaluator can't inspect artifacts**: The evaluator prompt receives artifact paths but can't read file contents, run tests, or verify actual output. Evaluation is based on metadata and logs.

5. **No authorization on task mutations**: Any agent can mark any task done/failed. There's no check that the agent assigned to a task is the one completing it (though `assigned` field tracks intent).

6. **Triage "done" verdict risk**: The LLM triage system can mark tasks as done based solely on log output analysis. If the log looks like progress but the work is incomplete, the task is incorrectly completed.
