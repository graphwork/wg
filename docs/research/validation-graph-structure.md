# Structural Validation in Task Graphs

Research into how task graph **structure** can enforce validation — catching bad work through graph topology rather than relying solely on agent self-reporting.

## 1. Current Patterns

### 1.1 Auto-Evaluate: Universal Structural Validation

The coordinator's `build_auto_evaluate_tasks()` (in `src/commands/service/coordinator.rs:347`) creates an `evaluate-{task-id}` task for **every** non-meta task when `auto_evaluate` is enabled. This is the primary structural validation mechanism.

**Empirical results from the current graph (725 tasks):**
- 328 evaluate tasks (45% of all tasks)
- **100% of non-meta work tasks have an evaluate successor**
- 0 implementation tasks lack an evaluation task
- All 328 evaluate tasks correctly depend on their target via the `after` field

**What gets excluded** (preventing infinite regress):
- Tasks tagged `evaluation`, `assignment`, or `evolution`
- Tasks assigned to human agents
- Abandoned tasks

**Evaluation tasks run inline** (`spawn_eval_inline` at line 492) — the coordinator forks `wg evaluate run <source-task>` directly instead of the full agent spawn pipeline, reducing overhead.

**Evaluation task outcomes:**
- 310 done (95%)
- 16 open (5%, waiting for source tasks still in progress)
- 2 failed (0.6%)

### 1.2 What Evaluations Check

The `wg evaluate` command (`src/commands/evaluate.rs`) builds an evaluator prompt that includes:
- Task title, description, skills, and verify criteria
- Agent identity (role, motivation)
- Artifacts produced
- Log entries
- Timing information

The evaluator produces a structured score (0.0–1.0) with dimensions: correctness, completeness, efficiency, style_adherence.

### 1.3 Other Structural Patterns

**Assign tasks** (68 total, 9% of graph): `assign-{task-id}` tasks perform agent selection. These are also auto-created by the coordinator and precede work tasks.

**Verify field**: The task schema supports a `verify` field (string instruction for agents), but **0 out of 725 tasks currently use it**. This is an underutilized mechanism.

**Deliverables field**: Similarly, `deliverables` (list of expected output files) exists in the schema but has **0 usage** across all tasks.

**Triage system** (`src/commands/service/triage.rs`): When an agent dies, the triage LLM assesses progress and issues a done/continue/restart verdict. This is runtime validation of agent work quality.

## 2. The Evaluate-* Pattern: Strengths and Gaps

### 2.1 Strengths

1. **Universal coverage**: Auto-evaluate ensures no task escapes review.
2. **Structural enforcement**: Evaluation is a graph property, not agent behavior — agents can't opt out.
3. **Inline execution**: Evaluation tasks use a lightweight spawn path, reducing cost.
4. **Agency integration**: Evaluation scores feed into the evolution system, creating double-loop learning.

### 2.2 Gaps

1. **Post-hoc only**: Evaluations run _after_ work is done. They can inform evolution but can't prevent bad work from being marked done.
2. **No gating**: An evaluation task that scores 0.2 doesn't block downstream work. The evaluate task is a _leaf_ — nothing depends on it.
3. **No remediation path**: When an evaluation finds poor work, there's no structural mechanism to trigger a retry or fix.
4. **Single dimension**: Every task gets the same evaluation prompt template. Tasks that modify code should be evaluated differently from research or documentation tasks.

## 3. Graph Linting Proposals

The existing `wg check` command (`src/check.rs`) performs 4 checks:
- Cycle detection
- Orphan reference detection
- Stale assignment detection
- Stuck blocked task detection

A `wg lint` command (or extension of `wg check`) could add validation-specific checks:

### 3.1 Implementation Without Validation Successor

**Rule**: Any task with tags/skills suggesting code modification (`implementation`, `coding`, `fix`, `refactor`) should have at least one validation successor (evaluate, validate, verify, test, review).

**Currently moot** because auto-evaluate gives 100% coverage, but valuable if auto-evaluate is disabled or for manual graph construction.

**Implementation**: Walk the successor graph for each impl-like task. If no successor has a validation-related tag or ID prefix, emit a warning.

### 3.2 Diamond Missing Integration Task

**Rule**: If tasks A→[B,C]→ and B and C have no common successor, warn about missing integration.

**Implementation**: For each set of tasks that share a predecessor but have no common successor, emit a warning. This catches "orphaned parallel workers" where results never get merged.

### 3.3 Evaluation Score Gating

**Rule (proposed)**: If `evaluate-X` scores below a threshold (e.g., 0.5), tasks that depend on X's outputs should be flagged.

**Implementation**: After evaluation completes, check the score. If below threshold, optionally:
- Create a `fix-X` task with the evaluation notes
- Block downstream tasks on the fix
- Add a loop edge back to X for retry

### 3.4 File Scope Overlap Detection

**Rule**: Parallel tasks (same predecessor, no ordering between them) should not modify the same files.

**Challenge**: The `deliverables` and `artifacts` fields are currently unused (0 tasks use either). File scope information would need to come from:
- Task descriptions (heuristic parsing)
- Deliverables field (if populated)
- Artifacts recorded after completion (too late for linting)

**Proposal**: Make deliverables a first-class concern:
1. `wg add --deliverable src/foo.rs` to declare expected outputs
2. Lint: warn when parallel tasks share deliverables
3. Post-hoc: after `wg artifact`, check if recorded artifacts overlap with concurrent tasks

### 3.5 Unbounded Chains Without Checkpoints

**Rule**: A pipeline of more than N tasks without any validation/review task should generate a warning.

**Rationale**: Long pipelines without intermediate validation accumulate errors. A chain of `design→impl-a→impl-b→impl-c→impl-d→deploy` should have at least one review checkpoint.

### 3.6 Cycle Without Convergence Criteria

**Rule**: A cycle (loop pattern) should have either `--max-iterations` or a verify/guard condition documented.

**Currently partially enforced** by the AGENT-GUIDE anti-pattern list but not by tooling.

## 4. Template Enforcement via Functions

The `wg func` system (`src/function.rs`, `src/plan_validator.rs`) already has structural constraint enforcement:

### 4.1 Existing Constraints (StructuralConstraints)

- `min_tasks` / `max_tasks`: task count bounds
- `required_skills`: skills that must appear across generated tasks
- `required_phases`: tags (phases) that must be present
- `forbidden_patterns`: tag combinations that are banned
- `allow_cycles` / `max_total_iterations`: cycle control
- `max_depth`: dependency chain limit

### 4.2 Missing Constraint: Required Validation Phase

The most impactful addition would be a **`require_validation_successor`** constraint (or making `required_phases` include a built-in "validate" phase). This would ensure every func template includes a validation step.

**Concrete example** — the `doc-sync` function has 10 tasks but no validation task; validation only comes from auto-evaluate, which runs after the function is complete. A constraint could require at least one task tagged "validate" or "review" in every function.

### 4.3 Proposed: Validation Task Injection

Functions could automatically inject validation tasks:

```yaml
# In function definition
validation:
  strategy: per-worker    # or "at-join", "final-only"
  template:
    title: "Validate {{parent.title}}"
    skills: [review]
    role_hint: analyst
```

This would structurally ensure that when `wg func apply` creates tasks, each worker (or the join point, or the final task) gets a validation successor.

### 4.4 The Sample Function Already Does This

The `sample_function()` test fixture in `src/commands/func_cmd.rs` shows the ideal pattern:
```
plan → implement → validate
```
This three-stage pipeline (plan, implement, validate) should be the default pattern that functions are encouraged to follow.

## 5. Dependency and File Scope Validation

### 5.1 Current State

There is **no file-scope overlap detection** in the codebase. The critical structural rule ("same files = sequential edges") is documented in the AGENT-GUIDE but not enforced by tooling.

The `deliverables` field exists in both `Task` and `TaskTemplate` but is unused across all 725 tasks.

### 5.2 Proposed: Deliverables-Based Overlap Detection

**Phase 1: Populate deliverables**
- Extend `wg add --deliverable path/to/file` to set expected outputs
- When agents run `wg artifact`, cross-reference against parallel tasks
- `wg func apply` should propagate template deliverables to created tasks

**Phase 2: Static overlap check**
- At graph modification time, check if any pair of parallel tasks (tasks that can run concurrently — i.e., no ordering edge between them) share deliverables
- Emit a warning: "Tasks X and Y both declare deliverable src/main.rs but have no ordering edge"

**Phase 3: Runtime conflict detection**
- When `wg artifact <task> <file>` is called, check if any in-progress task also has that file as a deliverable
- If so, log a warning to the task

### 5.3 Implementation Sketch

```rust
// In check.rs or a new lint.rs
pub fn check_parallel_file_overlap(graph: &WorkGraph) -> Vec<FileOverlap> {
    let mut overlaps = Vec::new();
    let parallel_pairs = find_parallel_task_pairs(graph);

    for (a, b) in parallel_pairs {
        let shared: Vec<_> = a.deliverables.iter()
            .filter(|d| b.deliverables.contains(d))
            .collect();
        if !shared.is_empty() {
            overlaps.push(FileOverlap {
                task_a: a.id.clone(),
                task_b: b.id.clone(),
                shared_files: shared,
            });
        }
    }
    overlaps
}
```

## 6. Specific Proposals (Prioritized)

### High Impact, Low Effort

1. **Populate `verify` field in executor prompt** — The executor already creates task prompts. Adding "Verify: run `cargo test`" to implementation tasks would give agents built-in self-checks. The field exists but is unused.

2. **Add `required_phases: ["validate"]` to func constraints** — For any function that creates implementation tasks, require a validation phase tag. The `plan_validator.rs` already enforces this.

3. **Extend `wg check` with `--lint` flag** — Add validation-focused checks without changing the default check behavior. Start with "diamond missing integrator" and "pipeline without checkpoint" rules.

### Medium Impact, Medium Effort

4. **Evaluation score gating** — When `evaluate-X` scores below 0.5, automatically create a `fix-X` task. This closes the feedback loop structurally.

5. **Deliverables field population in `wg add` and `wg func apply`** — Enable file-scope overlap detection by making deliverables standard practice.

6. **Func validation injection** — Allow function definitions to specify automatic validation task injection at worker, join, or final positions.

### High Impact, High Effort

7. **Full `wg lint` command** — A comprehensive graph linter that checks all structural validation rules, with configurable severity levels and `.wg/lint.toml` configuration.

8. **Runtime conflict detection** — Monitor artifact declarations in real-time and warn/block when parallel tasks claim the same files.

## 7. Architecture Decision

The key architectural question is: **should validation be advisory or enforcing?**

| Approach | Pro | Con |
|----------|-----|-----|
| **Advisory** (warnings via `wg lint`) | Non-disruptive, easy to adopt | Agents and humans can ignore warnings |
| **Enforcing** (block graph operations) | Guarantees validation | Can block legitimate edge cases |
| **Hybrid** (advisory by default, enforcing for functions) | Best of both worlds | More complex to implement |

**Recommendation**: Start with **advisory** (extend `wg check`, add `wg lint`), then add **enforcing** for `wg func apply` via StructuralConstraints. This matches the existing pattern where `wg check` warns and `plan_validator` enforces.

## 8. Summary

workgraph already has strong structural validation through auto-evaluate (100% coverage). The main gaps are:

1. **Evaluation is post-hoc and non-gating** — bad evaluations don't trigger remediation
2. **Unused fields** (`verify`, `deliverables`) that could enable self-checking and file overlap detection
3. **No graph linting** beyond basic checks (cycles, orphans, stale assignments)
4. **Functions lack validation enforcement** — the constraint system exists but "require validation task" isn't a built-in rule

The proposed improvements build on existing infrastructure (check module, plan_validator, func constraints) rather than introducing new systems. The highest-value change is closing the evaluation→remediation loop so that low-scoring evaluations structurally trigger fixes.
