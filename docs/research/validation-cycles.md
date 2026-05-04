# Validation in Cycle Iterations

**Date:** 2026-02-25
**Task:** research-validation-in
**Status:** Research complete

---

## 1. Current Cycle Mechanics

### How Cycles Work

workgraph cycles are **structural loops** in the task dependency graph. A cycle is a set of tasks connected by `after` edges that form a strongly connected component (SCC), detected via Tarjan's algorithm (`src/cycle.rs`). One task in the cycle is designated the **cycle header** — it carries `CycleConfig` and controls iteration behavior.

**Creating a cycle:**
```bash
wg add "Write" --id write --after revise --max-iterations 5
wg add "Review" --id review --after write
wg add "Revise" --id revise --after review
```

This creates the cycle `write → review → revise → write` with `write` as the header (it has `CycleConfig`). The `--after revise` plus `--max-iterations` on `write` auto-creates the back-edge (`src/commands/add.rs:205-208`).

### CycleConfig (src/graph.rs:8-17)

```rust
pub struct CycleConfig {
    pub max_iterations: u32,      // Hard cap
    pub guard: Option<LoopGuard>, // Condition for iteration
    pub delay: Option<String>,    // Delay between iterations
}
```

### LoopGuard variants (src/graph.rs:20-28)

| Variant | Semantics |
|---------|-----------|
| `Always` | Always iterate (up to max_iterations) |
| `IterationLessThan(n)` | Iterate while count < n |
| `TaskStatus { task, status }` | Iterate while task has given status |

### Iteration Lifecycle

When a task in a cycle completes (`wg done`), `evaluate_cycle_iteration()` (`src/graph.rs:933-978`) fires:

1. **All done?** — All cycle members must be `Status::Done`. If not, no iteration.
2. **Convergence check** — If no external guard is set, checks if header has `"converged"` tag. If so, stops.
3. **Max iterations** — If `loop_iteration >= max_iterations`, stops.
4. **Guard evaluation** — Evaluates `LoopGuard` against graph state.
5. **Re-activate** — Re-opens all members: sets `Status::Open`, clears `assigned`/timestamps, increments `loop_iteration`, optionally sets `ready_after` for delay.

### Convergence via `--converged` (src/commands/done.rs:64-113)

Agents signal convergence with `wg done <id> --converged`. This adds a `"converged"` tag to the task. On the next cycle evaluation, the tag causes step 2 to return early, halting the cycle.

**Guard authority** — When a non-trivial guard is set (not `Always`, not `None`), the `--converged` flag is **ignored** (`done.rs:70-113`). The guard is authoritative. This prevents agents from bypassing external validation by self-declaring convergence.

### Cycle-Aware Dispatch (src/query.rs:331-375)

`ready_tasks_cycle_aware()` exempts back-edge predecessors from readiness checks. This allows the cycle header to become ready on the first iteration even though its back-edge predecessor hasn't completed yet — but only if the header has `CycleConfig`.

---

## 2. Convergence Signals: Current State and Gaps

### Current signals

| Signal | Type | Authority |
|--------|------|-----------|
| `--converged` flag | Agent-driven | Self-declared, subjective |
| `max_iterations` | System-driven | Hard cap, no quality check |
| `LoopGuard::TaskStatus` | System-driven | Binary (task status match) |
| `LoopGuard::IterationLessThan` | System-driven | Equivalent to max_iterations |
| `LoopGuard::Always` | System-driven | Trivial (always iterate) |

### What's missing

**No quality-based convergence.** The system has no way to say "iterate until quality score > 8" or "iterate until tests pass." The `TaskStatus` guard can express "iterate while task X has status Y" but cannot express quality thresholds.

**No comparison between iterations.** The system tracks `loop_iteration` as a counter but does not retain or compare outputs across iterations. Each iteration's agent starts fresh (except for artifacts and logs preserved in workgraph).

**Subjective agent convergence.** The `--converged` flag relies entirely on the agent's judgment. The loop-convergence design doc (`docs/design/loop-convergence.md`) acknowledges this as the simplest approach but flags `ScoreAbove` guards and structured convergence criteria as future work.

**No feedback from evaluation into cycle control.** The auto-evaluation system (`build_auto_evaluate_tasks()` in `coordinator.rs:334-453`) creates `evaluate-{task-id}` tasks after completion. But these evaluations are **downstream** of the cycle — they run after the task completes and don't feed back into the cycle's iteration decision. The evaluation and cycle systems are explicitly described as "orthogonal" in the loop-convergence design doc (line 88-99).

---

## 3. Quality Ratchet: Detecting Regression Between Iterations

### The problem

In a cycle like `write → review → revise → write`, each iteration should produce better output than the last. But nothing currently prevents:
- An agent making destructive changes that undo prior work
- An iteration's output being objectively worse than the previous
- An agent re-doing already-completed work

### What the system preserves across iterations

1. **Logs** — Each iteration's log entries are preserved with timestamps and iteration numbers. Agents can access them via `wg log <task-id> --list` and `wg context`.
2. **Artifacts** — Files registered via `wg artifact` persist across iterations. However, their content changes as agents modify them.
3. **Git history** — If agents commit, the git log provides diffs between iterations.
4. **Output capture** — `.wg/output/{task-id}/` captures git diff, artifacts, and logs after completion (`capture_task_output()` in `src/commands/done.rs:200-208`).

### What's NOT preserved

- **Snapshot of quality score per iteration** — No per-iteration evaluation record.
- **Diff between iteration outputs** — No automatic comparison of iteration N vs N-1.
- **Quality metric over time** — No time series of quality measurements.

### Proposal: Per-Iteration Quality Snapshots

**Core idea:** After each cycle iteration completes, before deciding whether to re-activate, capture a quality snapshot. Compare it to the previous iteration's snapshot. If quality regressed, flag it.

**Implementation sketch:**

1. **New field on Task**: `iteration_scores: Vec<IterationScore>` where:
   ```rust
   pub struct IterationScore {
       pub iteration: u32,
       pub score: f64,            // 0.0-1.0
       pub evaluator: String,     // who scored it
       pub timestamp: String,
       pub notes: Option<String>,
   }
   ```

2. **New LoopGuard variant**: `ScoreAbove(f64)` — iterate until score exceeds threshold.

3. **Integration point**: In `reactivate_cycle()`, before re-opening members, check:
   - If `iteration_scores` has an entry for the current iteration
   - If the score meets or exceeds the guard threshold
   - If the score is >= the previous iteration's score (ratchet)

4. **Scoring mechanism**: Two options:
   - **Evaluation-driven**: The auto-evaluation system already creates `evaluate-{task-id}` tasks. Wire their output scores into `iteration_scores`.
   - **Guard-driven**: A `TaskStatus` guard already watches an external task. Extend it to watch a score on an external task.

**Trade-off analysis:**

| Approach | Pros | Cons |
|----------|------|------|
| Per-iteration eval | Rich signal, human-auditable | Adds latency (eval must complete before next iteration) |
| Simple pass/fail guard | Fast, no extra tasks | Binary, no nuance |
| Score threshold guard | Quantitative, automated | Requires evaluation infrastructure |
| Git diff regression check | Detects destructive changes | Coarse, doesn't measure quality |

**Recommendation:** Start with making evaluations **synchronous within cycles** (see section 4). A `ScoreAbove` guard can then reference the evaluation score.

---

## 4. Cycle-Specific Validation: Upfront Criteria

### Current state

Tasks can have a `verify` field (a freeform string describing verification criteria), but it's informational — it doesn't gate completion. The `exec` field can run a command, but it's a one-shot execution hint, not a cycle guard.

### Proposal: Validation Criteria on Cycles

Cycles should support **declarative convergence criteria** defined at creation time. This decouples the "when to stop" decision from subjective agent judgment.

#### Proposed criteria types

1. **Test-based**: "Iterate until `cargo test` passes"
   ```bash
   wg add "Fix" --after test --max-iterations 5 \
     --cycle-guard "exec:cargo test"
   ```
   - New `LoopGuard::Exec { command: String, expected_exit: i32 }`
   - Guard evaluates by running the command; iteration continues if exit code doesn't match
   - **Risk:** Exec guards introduce side effects into the cycle evaluation. Must be idempotent, fast, and sandboxed.

2. **Score-based**: "Iterate until evaluation score > 0.8"
   ```bash
   wg add "Write" --after review --max-iterations 5 \
     --cycle-guard "score:0.8"
   ```
   - New `LoopGuard::ScoreAbove(f64)`
   - Requires evaluation scores to be written back to the task (see section 3)
   - Clean separation: the guard reads a score; a separate evaluation task writes it

3. **Artifact-based**: "Iterate until specific file exists or contains pattern"
   ```bash
   wg add "Generate" --after validate --max-iterations 3 \
     --cycle-guard "file-exists:output/report.pdf"
   ```
   - New `LoopGuard::FileExists { path: String }`
   - Simple, filesystem-based check
   - Useful for code generation cycles

4. **Composite**: Multiple conditions ANDed together
   ```bash
   wg add "Refine" --after check --max-iterations 5 \
     --cycle-guard "exec:cargo test" \
     --cycle-guard "score:0.7"
   ```
   - `LoopGuard` becomes composable: `LoopGuard::All(Vec<LoopGuard>)`
   - All conditions must be met to stop

#### Recommended priority

| Priority | Guard type | Complexity | Value |
|----------|-----------|------------|-------|
| P0 | `ScoreAbove(f64)` | Medium | High — ties evaluation to cycle control |
| P1 | `Exec { command }` | Medium | High — enables test-driven cycles |
| P2 | `FileExists { path }` | Low | Medium — useful for generation workflows |
| P3 | `All(Vec<LoopGuard>)` | Low | Medium — composability |

---

## 5. Infinite Loop Prevention: Beyond max_iterations

### Current safeguards

1. **`max_iterations`** — Hard cap on cycle count. Required on every cycle header (enforced by convention per AGENT-GUIDE.md, but not by the system).
2. **`--converged` flag** — Agent-driven early termination.
3. **Guard conditions** — `TaskStatus`, `IterationLessThan` can prevent iteration.
4. **`wg check`** — Detects unconfigured cycles (cycles without `CycleConfig`) and warns about deadlocks.

### Gaps in infinite loop prevention

1. **No mandatory max_iterations.** A cycle can be created without `--max-iterations` if back-edges are added manually (e.g., `wg edit --add-after`). The system doesn't enforce that every cycle has a `CycleConfig`. `wg check` warns but doesn't block.

2. **No cost accounting.** A 100-iteration cycle with expensive agents can burn significant resources without any budget check.

3. **No progress detection.** The system can't detect that iterations are producing identical or near-identical outputs (oscillation).

4. **No timeout.** There's no wall-clock limit on how long a cycle can run.

5. **Dead agent in cycle.** If an agent dies mid-cycle, the triage system (`src/commands/service/triage.rs`) detects the dead PID and marks the task as failed. But a failed task in a cycle prevents further iteration (all members must be Done). The cycle effectively stalls.

### Proposals

#### A. Mandatory CycleConfig enforcement

Make it a hard error (not just a warning) for a cycle to exist without `CycleConfig` on exactly one member. Enforce in `wg check --strict` or at graph mutation time.

**Implementation:** In `add_node()` or `save_graph()`, compute cycle analysis and verify every detected SCC has exactly one member with `CycleConfig`.

**Trade-off:** This would break the ability to have "accidental" cycles that deadlock intentionally (a legitimate pattern for signaling errors). Better to make it a `wg check` error level (not just warning) and have the coordinator refuse to dispatch unconfigured cycles.

#### B. Cost budget per cycle

Add `max_cost: Option<f64>` to `CycleConfig`. The coordinator sums `token_usage` from all completed iterations and halts the cycle when the budget is exceeded.

```rust
pub struct CycleConfig {
    pub max_iterations: u32,
    pub guard: Option<LoopGuard>,
    pub delay: Option<String>,
    pub max_cost: Option<f64>,  // new: total cost budget in dollars
}
```

**Implementation:** In `reactivate_cycle()`, before re-opening members, sum `token_usage` across all members across all iterations. If sum > budget, don't re-activate.

#### C. Oscillation detection

Track a hash of the cycle's output across iterations. If the hash repeats (output is identical to a previous iteration), halt the cycle.

**Implementation sketch:**
1. After each iteration completes, compute a digest of all artifacts.
2. Store digests in `iteration_digests: Vec<String>` on the cycle header.
3. Before re-activating, check if the current digest matches any previous one.

**Trade-off:** Simple but coarse. Two iterations can produce identical file content while making meaningful progress in logs/understanding. Better as a warning than a hard stop.

#### D. Wall-clock timeout

Add `timeout: Option<String>` to `CycleConfig` (e.g., `"2h"`, `"1d"`). Record `cycle_started_at` on the first iteration. Halt if elapsed time exceeds timeout.

```rust
pub struct CycleConfig {
    // ...existing...
    pub timeout: Option<String>,       // new: wall-clock limit
    pub cycle_started_at: Option<String>, // new: when first iteration began
}
```

#### E. Failed-task recovery in cycles

When a task in a cycle fails, the cycle stalls (all members must be Done to iterate). Options:
1. **Auto-retry** — If the task has `max_retries`, the coordinator retries it. Already supported.
2. **Skip-and-iterate** — Allow the cycle to iterate even if one member failed. This would require a new cycle policy (e.g., `on_failure: skip | halt | retry`).
3. **Partial re-activation** — Only re-open the failed task, not all members.

**Recommendation:** Option 1 (auto-retry) is already in the system. For cycles specifically, add a `on_failure` policy to `CycleConfig`:

```rust
pub enum CycleFailurePolicy {
    Halt,              // default: stall the cycle
    RetryThenHalt,     // retry the failed task; if retry exhausted, halt
    SkipAndIterate,    // mark failed task as Done-with-failure, continue cycle
}
```

---

## 6. Synthesis: How Validation and Cycles Should Interact

### The feedback loop gap

The fundamental gap is that **evaluation runs after the cycle, not within it**. The auto-evaluation system creates a downstream task that scores the agent's work, but this score doesn't feed back into the cycle's iteration decision.

The desired architecture:

```
Iteration N
  → Agent completes work
  → Validate (tests, eval, criteria check)
  → Score recorded on task
  → Cycle guard checks score
  → If score < threshold: re-activate (iterate)
  → If score ≥ threshold: converge
```

### Concrete proposal: Evaluation-Gated Cycles

#### Design

1. **New LoopGuard variant**: `ScoreAbove { threshold: f64, source: ScoreSource }`
   ```rust
   pub enum ScoreSource {
       SelfEvaluation,           // score from evaluate-{task-id}
       ExternalTask(String),     // score from a specific task
   }
   ```

2. **Wire evaluation scores into cycle control**:
   - When `evaluate-{task-id}` completes, its score is written to the original task's `iteration_scores`.
   - `evaluate_cycle_iteration()` reads `iteration_scores` and checks against the `ScoreAbove` guard.

3. **Evaluation must complete before cycle re-activates**:
   - Currently, `evaluate_cycle_iteration()` fires in `wg done` (synchronous). The evaluation task hasn't even started at this point.
   - Change: Move cycle evaluation to the **coordinator tick** rather than `wg done`. The coordinator can check: "all cycle members done AND evaluation complete" before re-activating.
   - This is a significant architectural change but necessary for evaluation-gated cycles.

4. **Fallback**: If no evaluation is configured, `ScoreAbove` guard blocks forever. Require `auto_evaluate: true` when `ScoreAbove` is used, or document the dependency.

#### Migration path

| Phase | Change | Risk |
|-------|--------|------|
| 1 | Add `IterationScore` struct and `iteration_scores` field to Task | Zero — additive |
| 2 | Wire `wg evaluate run` to write scores into `iteration_scores` | Low — extends existing eval |
| 3 | Add `ScoreAbove` LoopGuard variant | Low — new variant, no existing behavior change |
| 4 | Move cycle re-activation to coordinator tick (deferred evaluation) | Medium — changes timing of re-activation |
| 5 | Add quality ratchet (score must not decrease) | Low — additive check in reactivate_cycle |
| 6 | Add `Exec` guard for test-driven convergence | Medium — side effects in guard evaluation |

### What NOT to do

- **Don't make all cycles require evaluation.** Many cycles are simple (draft → review → revise) and work fine with agent-driven convergence.
- **Don't block `--converged` when evaluations exist.** Agent convergence is a valid signal even when eval is enabled. They're complementary.
- **Don't add complexity for the common case.** Most cycles are 2-3 iterations. The validation infrastructure should be opt-in, not mandatory.
- **Don't parse agent output for convergence markers.** This was considered and rejected in `docs/design/loop-convergence.md` — fragile and unstructured.

---

## 7. Summary of Proposals

| # | Proposal | Priority | Complexity | Impact |
|---|----------|----------|------------|--------|
| 1 | `LoopGuard::ScoreAbove` — score-based convergence | P0 | Medium | Ties evaluation to cycle control |
| 2 | Per-iteration quality snapshots (`iteration_scores`) | P0 | Low | Foundation for score-based guards |
| 3 | `LoopGuard::Exec` — command-based convergence | P1 | Medium | Test-driven cycles |
| 4 | Deferred cycle evaluation in coordinator | P1 | High | Required for eval-gated cycles |
| 5 | Quality ratchet (score monotonicity check) | P2 | Low | Prevents regression |
| 6 | Cost budget per cycle (`max_cost`) | P2 | Low | Resource protection |
| 7 | Oscillation detection | P3 | Medium | Detects unproductive cycling |
| 8 | `CycleFailurePolicy` enum | P3 | Medium | Resilient cycles |
| 9 | Mandatory CycleConfig enforcement | P3 | Low | Prevents accidental deadlocks |
| 10 | Wall-clock timeout | P3 | Low | Safety net |

### Key architectural insight

The central tension is that **cycle re-activation currently happens synchronously in `wg done`**, but evaluation is asynchronous (a downstream task). To bridge this gap, cycle re-activation should be deferred to the coordinator, which can wait for evaluation to complete before deciding whether to iterate. This is the highest-impact change and should be designed carefully.

### Files referenced

| File | Relevance |
|------|-----------|
| `src/graph.rs:8-28` | CycleConfig, LoopGuard definitions |
| `src/graph.rs:933-1066` | evaluate_cycle_iteration(), reactivate_cycle() |
| `src/commands/done.rs:15-212` | Task completion, convergence handling, guard authority |
| `src/commands/add.rs:126-148` | CycleConfig construction |
| `src/query.rs:326-375` | Cycle-aware readiness |
| `src/commands/service/coordinator.rs:334-453` | Auto-evaluation task creation |
| `src/commands/service/triage.rs:165-168` | Cycle iteration in triage |
| `src/service/executor.rs:272-298` | Cycle info in agent prompts |
| `src/cycle.rs` | Tarjan SCC, Havlak loop nesting |
| `docs/design/loop-convergence.md` | `--converged` design rationale |
| `docs/design/cycle-delay-semantics.md` | Delay semantics proposal |
| `docs/design/spec-cycle-integration.md` | Full cycle integration spec |
