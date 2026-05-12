# Validation Improvement Initiative: Synthesis & Action Plan

**Date:** 2026-02-25
**Task:** synthesize-validation-research
**Sources:** 6 research artifacts from parallel investigation

---

## Executive Summary

Six parallel research threads investigated validation in wg from every angle — existing mechanisms, agent behavior, graph structure, evaluation quality, cycle iteration, and agent teaching. Five cross-cutting findings emerge:

1. **Agents don't validate because nobody tells them to.** The prompt template, AGENT-GUIDE.md, and CLAUDE.md contain zero validation guidance. The word "validate" appears nowhere in agent-facing prompts. Code agents validate ~60% of the time (model training), but documentation and research agents almost never do.

2. **Evaluations are post-hoc, non-gating, and unreliable.** The auto-evaluation system has 100% coverage (every task gets evaluated), but scores are purely informational — a 0.2 score doesn't block downstream work, trigger retries, or alert anyone. Worse, 48.5% of evaluations score ≥0.9, inter-evaluator spread exceeds 0.5 for 9 tasks, and confirmed false positives exist where evaluators hallucinated code that never existed.

3. **The `verify` and `deliverables` fields exist but are invisible.** Both fields are defined in the Task struct, but 0 of 725 tasks use either one. The `verify` field is never surfaced in agent prompts. These are the scaffolding for a validation system that was designed but never activated.

4. **Evaluation and cycle systems are disconnected.** Evaluation scores don't feed back into cycle iteration decisions. There's no "iterate until quality is sufficient" pattern. The `--converged` flag relies entirely on subjective agent judgment, and there are no quality-based loop guards.

5. **Structural validation infrastructure exists and is strong — but only for edge cases.** Plan validation (18 tests, 8 constraint types), cycle guards, self-convergence prevention, and auto-assign regress guards are all well-implemented. The gap is in the common case: an agent completing a normal task with no quality gate whatsoever.

---

## Gap Analysis

Prioritized by impact — how much bad work reaches downstream consumers.

### Critical (allows bad work to propagate)

| # | Gap | Impact | Source |
|---|-----|--------|--------|
| G1 | **No validation guidance in agent prompts** | Agents skip validation on ~40-90% of non-code tasks | Teaching Agents §1, Self-Checks §1 |
| G2 | **`wg done` has no quality gate** | Any agent can mark any task done without producing deliverables, passing tests, or meeting verify criteria | Current Mechanisms §3, Self-Checks §1.4 |
| G3 | **Evaluation scores are informational only** | Low scores don't trigger retries, block downstream tasks, or flag issues; the feedback loop is broken | Current Mechanisms §1, Structural §2.2 |
| G4 | **Evaluator can't verify ground truth** | Evaluator sees artifact paths but not content; doesn't read files, run tests, or verify claims; confirmed hallucination cases | Evaluation Quality §5, §7 |

### High (reduces quality signal reliability)

| # | Gap | Impact | Source |
|---|-----|--------|--------|
| G5 | **`verify` field invisible to agents** | Task creators can specify verification criteria but agents never see them | Teaching Agents §1.1, Self-Checks §1.4 |
| G6 | **Evaluation positive bias** | Mean 0.857, 48.5% score ≥0.9; evaluator is too lenient, reducing discriminative power | Evaluation Quality §1 |
| G7 | **Temporal state divergence in evaluations** | Evaluator sees repo at eval time, not completion time; post-completion changes cause false negatives and false positives | Evaluation Quality §4 |
| G8 | **Cycles lack quality-based convergence** | No "iterate until quality > threshold" pattern; convergence relies on subjective agent judgment or fixed iteration counts | Cycles §2 |

### Moderate (creates maintenance burden or risk)

| # | Gap | Impact | Source |
|---|-----|--------|--------|
| G9 | **`wg check` is manual** | Structural checks (orphans, stale assignments, stuck blocked) don't run automatically | Current Mechanisms §5 |
| G10 | **No file-scope overlap detection** | Parallel tasks can silently overwrite each other's files; `deliverables` field unused | Structural §5 |
| G11 | **Triage "done" verdict risk** | LLM triage can mark incomplete tasks as done based on misleading logs | Current Mechanisms §4 |
| G12 | **No authorization on task mutations** | Any agent can mark any task done; no check that the assigned agent is the one completing it | Current Mechanisms §3 |

---

## Recommendations

### Tier 1: Immediate (prompt/doc changes, no code)

#### R1. Add validation section to the agent prompt template
**Addresses:** G1, G2
**Effort:** ~10 lines in `src/service/executor.rs`
**Impact:** Every spawned agent sees validation instructions

Add a "Validate your work" step to `REQUIRED_WORKFLOW_SECTION` between artifact recording and task completion. Include task-type-specific guidance (code: build + test; research: re-read description + verify references; docs: re-read files). Add "Log your validation: `wg log <id> 'Validated: ...'`". This is the single highest-impact change across all research.

#### R2. Add validation section to AGENT-GUIDE.md (§4.5)
**Addresses:** G1
**Effort:** ~30 lines of documentation
**Impact:** Persistent reference for agents and humans

New section "Validation — the final step before done" with a task-type validation table, log format, and anti-pattern entry for "Skipping validation."

#### R3. Add verification instructions to evaluator prompt
**Addresses:** G4, G6
**Effort:** ~10 lines in `src/agency/prompt.rs`
**Impact:** Evaluators catch fabrication and missing artifacts

Add explicit instructions: "Read each artifact file and verify it contains task-relevant content. Cross-reference log claims with observable evidence. If artifacts are missing, cap correctness at 0.3."

### Tier 2: Short-term (small code changes, new functionality)

#### R4. Surface `task.verify` in agent prompts
**Addresses:** G5
**Effort:** ~20 lines across `executor.rs` and `TemplateVars`
**Impact:** Task creators can specify verification criteria agents actually see

Add `task_verify: Option<String>` to `TemplateVars`. In `build_prompt()`, render it as a "Verification Required" section. This activates an existing but dormant field.

#### R5. Include git diff in evaluator prompt
**Addresses:** G4, G7
**Effort:** ~30 lines in `evaluate.rs`
**Impact:** Evaluators see ground truth (actual code changes) instead of relying on self-reported logs

Run `git diff <start_commit>..HEAD -- <artifact_paths>` at task completion time and include the output in the evaluator prompt. This also partially addresses temporal divergence (G7) by capturing the diff at completion time.

#### R6. Soft validation warning in `wg done`
**Addresses:** G2
**Effort:** ~15 lines in `done.rs`
**Impact:** Visible nudge when agents skip validation logging

Print a warning (not a hard block) when `wg done` is called and no task log entry contains "validat" (case-insensitive). Reinforces the norm without breaking workflows.

#### R7. Add `validation_discipline` dimension to evaluation rubric
**Addresses:** G1, G6
**Effort:** ~15 lines in `prompt.rs`
**Impact:** Creates selection pressure via evolution — agents that validate score higher and survive

New evaluation dimension: 1.0 = ran tests + reviewed diff + logged results; 0.4 = no explicit validation but work appears correct; 0.0 = no validation with detectable errors.

### Tier 3: Medium-term (new features, architectural changes)

#### R8. Score-gated cycle convergence (`LoopGuard::ScoreAbove`)
**Addresses:** G8
**Effort:** ~200 lines across `graph.rs`, `done.rs`, `coordinator.rs`
**Impact:** Enables "iterate until quality is sufficient" — ties evaluation to cycle control

New loop guard variant: `ScoreAbove(f64)`. Requires per-iteration quality snapshots (`iteration_scores` field on Task) and deferred cycle re-activation in the coordinator (evaluation must complete before deciding whether to iterate).

#### R9. Evaluation score gating for downstream tasks
**Addresses:** G3
**Effort:** ~150 lines in `coordinator.rs`
**Impact:** Closes the feedback loop — low evaluation scores trigger remediation

When `evaluate-X` scores below a configurable threshold (e.g., 0.5), automatically create a `fix-X` task with the evaluation notes. Optionally block downstream tasks until the fix completes. This converts evaluations from advisory to actionable.

#### R10. Run `wg check` automatically in the coordinator
**Addresses:** G9
**Effort:** ~30 lines in `coordinator.rs`
**Impact:** Structural problems detected and logged continuously

Run `check_all()` on each coordinator tick (or every N ticks). Log warnings. Don't block operations, but surface issues in the coordinator log.

#### R11. `wg validate` command
**Addresses:** G2
**Effort:** ~200 lines, new command
**Impact:** Makes validation the path of least resistance for agents

Detects project type (Cargo.toml → Rust, package.json → Node), runs appropriate test command, logs the result automatically. Reduces validation from 3 steps to 1.

---

## Implementation Tasks

Ready to be added with `wg add`. Ordered by dependency and priority.

### Task 1: Add validation guidance to prompt template (R1)
```
Title: Add validation section to agent prompt template
Description: Add a "Validate your work" step to REQUIRED_WORKFLOW_SECTION in src/service/executor.rs. Insert between artifact recording (step 2) and task completion (step 3). Include: (1) code tasks: cargo build + cargo test, (2) research tasks: re-read description + verify references, (3) all tasks: log validation with wg log. Also update the "Important" section to say "Validate BEFORE running wg done."
Skills: rust, implementation
Verify: cargo build succeeds. Read the prompt output of a spawned agent (via wg show) and confirm validation instructions appear.
```

### Task 2: Surface task.verify in agent prompts (R4)
```
Title: Surface task.verify field in agent prompts
Description: Add task_verify: Option<String> to TemplateVars in src/service/executor.rs. Populate from task.verify. In build_prompt(), render as "## Verification Required\nBefore marking done, you MUST verify:\n{verify}" when present. This activates the dormant verify field so task creators' verification criteria reach agents.
Skills: rust, implementation
Verify: Create a task with --verify "run cargo test". Spawn an agent. Confirm the agent's prompt contains the verification section.
```

### Task 3: Improve evaluator prompt for ground truth (R3 + R5)
```
Title: Improve evaluator prompt with verification instructions and git diff
Description: Two changes to the evaluation system: (1) In src/agency/prompt.rs, add instructions to the evaluator: "Read each artifact file. Cross-reference log claims with evidence. If artifacts are missing, cap correctness at 0.3." (2) In src/commands/evaluate.rs or done.rs output capture, include a git diff of artifacts at completion time in the evaluator prompt. This gives evaluators ground truth instead of relying on self-reported logs.
Skills: rust, implementation
Verify: Run wg evaluate on a test task. Confirm the evaluator prompt includes verification instructions and diff content.
```

### Task 4: Add validation_discipline to evaluation rubric (R7)
```
Title: Add validation_discipline dimension to evaluation rubric
Description: In src/agency/prompt.rs evaluator rubric, add a new dimension: validation_discipline (0.0-1.0). Scoring: 1.0=ran tests + reviewed diff + logged validation; 0.7=tests run but not logged; 0.4=no explicit validation but work correct; 0.0=no validation with errors. Update weighting to include this dimension. This creates evolutionary pressure for validation behavior.
Skills: rust, implementation
Verify: Run wg evaluate on a task. Confirm output JSON includes validation_discipline dimension.
```

### Task 5: Soft validation warning in wg done (R6)
```
Title: Add soft validation warning to wg done
Description: In src/commands/done.rs, after marking a task done, check whether any log entry contains "validat" (case-insensitive). If not, print to stderr: "Tip: Log validation steps before wg done (e.g., wg log <id> 'Validated: tests pass')". Warning only — never block completion. This reinforces the validation norm.
Skills: rust, implementation
Verify: Mark a task done without validation logs — confirm warning appears. Mark another done with a "Validation: ..." log — confirm no warning.
```

---

## Success Metrics

### Leading indicators (measurable within weeks)

| Metric | Current Baseline | Target | How to Measure |
|--------|-----------------|--------|----------------|
| % of task logs containing "validat" | ~15% (estimated from research) | >60% | `grep -ri "validat" .wg/tasks/*/log* \| wc -l` vs total tasks |
| % of code tasks with build/test in logs | ~60% | >90% | Search task logs for "cargo test", "cargo build", "test pass" |
| Evaluation dimension: validation_discipline | N/A (doesn't exist yet) | Mean >0.7 | Parse evaluation JSON for new dimension |

### Lagging indicators (measurable over months)

| Metric | Current Baseline | Target | How to Measure |
|--------|-----------------|--------|----------------|
| Evaluation score spread (multi-eval tasks) | 9 tasks with spread >0.5 | <3 tasks with spread >0.5 | Analyze `.wg/agency/evaluations/` |
| False positive rate (high score on bad work) | ~4 confirmed cases | 0 | Manual audit of high-scoring evaluations |
| Mean evaluation score | 0.857 | 0.80-0.85 (slight decrease = more honest) | Statistical analysis of evaluation JSONs |
| Downstream task failures from bad upstream | Unknown (not tracked) | Tracked and decreasing | Add tracking in coordinator |

### Quality of validation itself

| What to watch | Why | How |
|--------------|-----|-----|
| "Validation: all checks passed" without evidence | Agents gaming the validation log requirement | Evaluators should verify validation claims (R3) |
| Validation log present but tests actually failing | False confidence | Automated `wg validate` command (R11) would catch this |
| Evaluation scores clustering at 0.9+ despite validation_discipline | Rubric not working | Calibration set of reference evaluations |

---

## Cross-Cutting Themes

### Theme 1: Activate dormant infrastructure
The `verify` field, `deliverables` field, and structural constraint system are already built but unused. The fastest wins come from activating what exists, not building new systems. R4 (surface verify) and making `deliverables` standard in `wg add` and `wg func apply` cost very little relative to their impact.

### Theme 2: Close the evaluation feedback loop
Today: task → done → evaluate → score → (nothing). The score floats in the void. Two changes close the loop:
- **Short-term (R7):** Score validation_discipline → evolution selects for validators
- **Medium-term (R9):** Low scores trigger fix tasks → bad work gets remediated structurally

### Theme 3: Give evaluators ground truth
The evaluator is only as good as its evidence. Today it sees paths and logs — metadata that agents can fabricate. R3 (verification instructions) and R5 (git diff) give evaluators observable evidence instead of self-reports. This simultaneously addresses positive bias (G6), false positives (G4), and temporal divergence (G7).

### Theme 4: Make validation the path of least resistance
Each step added to the agent workflow is a step that might be skipped. The `wg validate` command (R11) reduces validation to a single command. The prompt template changes (R1) make validation feel as natural as `wg done`. The soft warning (R6) creates a social norm. Together, these make validating easier than not validating.

---

## Implementation Order

```
Phase 1 (Immediate - prompts and docs):
  R1: Prompt template validation section
  R2: AGENT-GUIDE §4.5
  R3: Evaluator verification instructions

Phase 2 (Short-term - small code changes):
  R4: Surface task.verify in prompts
  R5: Git diff in evaluator prompt
  R6: Soft wg done warning
  R7: validation_discipline dimension

Phase 3 (Medium-term - new features):
  R8: ScoreAbove loop guard
  R9: Evaluation score gating
  R10: Automatic wg check in coordinator
  R11: wg validate command
```

Phases 1 and 2 can be parallelized internally (R1-R3 are independent; R4-R7 are independent). Phase 3 items depend on Phase 2 being in place (especially R8 depending on R7, and R9 depending on R5).

---

## Appendix: Research Source Summary

| Research | Key Finding | Key Recommendation |
|----------|------------|-------------------|
| [Current Mechanisms](validation-current-mechanisms.md) | 18 mechanisms, 8 strong / 5 moderate / 3 weak; no quality gate on `wg done` | Close the `wg done` quality gap |
| [Agent Self-Checks](validation-agent-self-checks.md) | Zero validation guidance anywhere; `verify` field unused | Add prompt template guidance + revive verify field |
| [Graph Structure](validation-graph-structure.md) | 100% auto-evaluate coverage but post-hoc and non-gating; 0/725 tasks use verify or deliverables | Evaluation score gating + graph linting |
| [Evaluation Quality](validation-evaluation-quality.md) | Mean 0.857, 48.5% ≥ 0.9; confirmed false positives via hallucination; temporal divergence | Evaluator ground truth (diffs) + verification instructions |
| [Cycles](validation-cycles.md) | Evaluation and cycles are orthogonal; no quality-based convergence | ScoreAbove guard + deferred cycle evaluation |
| [Teaching Agents](validation-teaching-agents.md) | Zero validation in prompts/guide/docs; ~60% code agents self-validate; ~10% doc agents | Prompt changes + AGENT-GUIDE section + wg validate command |
