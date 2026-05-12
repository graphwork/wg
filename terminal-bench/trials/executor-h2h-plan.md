# Head-to-Head Executor Comparison: Trial Plan

**Date:** 2026-04-05
**Task:** tb-trial-design
**Purpose:** Controlled comparison of old executor vs. enhanced executor (decomposition templates, error recovery, state injection, context enrichment) using Terminal Bench tasks.

---

## 1. Conditions

### Condition A — Bare Agent (No wg)

The pure baseline. A Claude agent receives only the task description and standard tools. No wg context, no decomposition guidance, no test discovery, no verification gates.

- **Executor:** Claude CLI (`claude --model {model} --print`)
- **Prompt:** Task description only, no `wg` CLI awareness
- **Tools:** Standard Claude Code tools (Read, Edit, Write, Bash, Glob, Grep)
- **Decomposition guidance:** None
- **Test discovery:** None
- **Verification gate:** None
- **Turn limit:** Unlimited (30-min wall-clock timeout)

### Condition B — Agent + wg, Old Executor

The wg executor as it existed **before** the five improvement tracks were merged. This is the executor with:
- Static `AUTOPOIETIC_GUIDANCE` (generic decomposition patterns, not task-adaptive)
- No pre-task test discovery (no `discover_test_files()` scanning)
- Inline verification only (`verify_mode = "inline"` — same agent verifies its own work)
- No task complexity classification (`classify_task_complexity()` absent)
- No `build_decomposition_guidance()` function
- Context injection for non-Claude models absent (`read_wg_guide()` not wired)

**Baseline config** (`.wg/config.toml` overrides):
```toml
[coordinator]
decomp_guidance = false       # disables adaptive decomposition templates
auto_test_discovery = false   # disables pre-spawn test file scanning
verify_mode = "inline"        # same agent verifies (no separate-agent verification)
```

**Git reference:** The old behavior is equivalent to any commit before `97efebd4` (impl-executor-test-discovery / impl-executor-decomp-templates). However, since the new features are gated by config flags, simply setting the flags above on the current codebase reproduces the old behavior without checking out an older commit. This is preferable because it isolates the executor improvements as the only variable.

- **Executor:** `wg service start` (coordinator dispatches agents)
- **Prompt:** Full wg agent prompt with `REQUIRED_WORKFLOW_SECTION`, `GIT_HYGIENE_SECTION`, `AUTOPOIETIC_GUIDANCE` (static), `GRAPH_PATTERNS_SECTION`
- **Tools:** Standard Claude Code tools + `wg` CLI
- **Decomposition guidance:** Static (generic patterns in `AUTOPOIETIC_GUIDANCE`)
- **Test discovery:** None
- **Verification gate:** Inline only (`wg done` runs verify command in same agent context)

### Condition C — Agent + wg, Enhanced Executor

The current executor with all five improvement tracks enabled (the defaults since `97efebd4`):

1. **Adaptive decomposition templates** (`decomp_guidance = true`): `classify_task_complexity()` categorizes tasks as Atomic/Pipeline/FanOut/Integration/Ambiguous, then `build_decomposition_guidance()` injects pattern-matched templates.
2. **Pre-task test discovery** (`auto_test_discovery = true`): `discover_test_files()` scans the project for test files (Python, Rust, JS patterns) and injects them into the agent prompt as `## Discovered Test Files`. Auto-generates `--verify` gates from discovered tests.
3. **Separate-agent verification** (`verify_mode = "separate"`): Verification runs in a new agent context (different conversation/context window), preventing false-PASS rates where the implementation agent rubber-stamps its own work.
4. **Context injection for non-Claude models**: `read_wg_guide()` injects `.wg/wg-guide.md` content for native executor paths.
5. **F prompt fix**: `build_condition_f_prompt()` uses `cat` to read tests directly instead of `find`.

**Enhanced config** (`.wg/config.toml` overrides):
```toml
[coordinator]
decomp_guidance = true         # adaptive decomposition templates
auto_test_discovery = true     # pre-spawn test file scanning + auto --verify
verify_mode = "separate"       # separate agent verifies (different context window)
```

- **Executor:** `wg service start` (coordinator dispatches agents)
- **Prompt:** Full wg agent prompt with adaptive decomposition, discovered test files section, and all five improvement sections
- **Tools:** Standard Claude Code tools + `wg` CLI
- **Decomposition guidance:** Adaptive (task-specific templates based on complexity classification)
- **Test discovery:** Enabled (discovered test files injected into prompt, auto-verify gates)
- **Verification gate:** Separate agent (new context window for verification)

---

## 2. Task Selection

### Selection Criteria

Tasks were selected to stress the features that differentiate the enhanced executor from the old one:
1. **Decomposition matters:** Multi-step tasks where breaking work into dependent subtasks should improve outcomes
2. **Error recovery matters:** Tasks with non-obvious failure modes where iterative debugging and test discovery provide an edge
3. **Verification matters:** Tasks where self-verification blind spots (false-PASS) have been documented in prior experiments
4. **Difficulty range:** Mix of medium and hard tasks. Trivially easy tasks (all conditions pass 100%) are excluded — they can't differentiate.

### Selected Tasks

| # | Task ID | Category | Prior A Pass Rate | Why Selected |
|---|---------|----------|-------------------|--------------|
| 1 | `build-cython-ext` | building | 33% | **Multi-step build pipeline.** Prior D trials failed on `test_ccomplexity` due to self-verification blind spot. Enhanced executor's separate-agent verification and test discovery should catch this. Decomposition templates should produce better build pipelines. |
| 2 | `cancel-async-tasks` | async-programming | 33% | **Single-function with subtle edge cases.** `test_tasks_cancel_above_max_concurrent` catches a concurrency edge case that all self-verification conditions missed. Tests whether test discovery (finding the verifier tests) closes the gap. Also tests whether the enhanced executor correctly classifies this as ATOMIC and avoids counterproductive decomposition (E scored 0% by decomposing this). |
| 3 | `overfull-hbox` | debugging | 33% | **Debugging with constrained solution space.** LaTeX overfull hbox fix without modifying synonym definitions. Prior D trials had 0 verification iterations (couldn't find what to verify). Tests whether test discovery and adaptive verification guidance help on tasks with non-obvious test criteria. |
| 4 | `regex-log` | text-processing | 33% | **Atomic task that should NOT be decomposed.** E scored 0% by fragmenting a holistic regex problem into subtasks. Tests whether adaptive decomposition correctly classifies as ATOMIC. Also tests whether test discovery (reading `test_outputs.py` for expected regex matches) helps converge on the correct regex faster. |
| 5 | `custom-memory-heap-crash` | debugging | 67% | **Multi-step C debugging.** Requires compiling in both debug and release modes, cannot modify protected files. Tests decomposition quality (pipeline: diagnose → fix → compile debug → compile release → test) and error recovery (compilation errors need iterative fixing). |
| 6 | `merge-diff-arc-agi-task` | reasoning | 67% | **Multi-step reasoning task.** E scored 100% on this (vs 67% for A' and D) because decomposition into git setup → data parsing → algorithm → testing genuinely helped. Tests whether the enhanced executor's adaptive decomposition achieves similar benefits without E's overhead. |
| 7 | `nginx-request-logging` | server-config | 33% | **System configuration with verification.** D scored 100% here thanks to its verification loop catching config issues. Tests whether separate-agent verification maintains this advantage while improving on tasks where D's self-verification had blind spots. |

### Task Coverage Matrix

| Executor Feature | Tasks That Stress It |
|-----------------|---------------------|
| Adaptive decomposition (classify as Atomic) | `cancel-async-tasks`, `regex-log` |
| Adaptive decomposition (Pipeline/FanOut templates) | `build-cython-ext`, `custom-memory-heap-crash`, `merge-diff-arc-agi-task` |
| Pre-task test discovery | `cancel-async-tasks`, `regex-log`, `overfull-hbox`, `build-cython-ext` |
| Separate-agent verification | `build-cython-ext`, `cancel-async-tasks`, `nginx-request-logging` |
| Error recovery / iterative debugging | `custom-memory-heap-crash`, `overfull-hbox`, `regex-log` |
| Context enrichment (full prompt) | All tasks (baseline B vs. enhanced C) |

---

## 3. Trial Design

### Trials Per Condition

**3 trials per task per condition** (total: 7 tasks x 3 trials x 3 conditions = 63 trials).

Rationale:
- 3 trials matches the prior pilot design, enabling direct comparison with existing A'/D/E data.
- With 7 tasks x 3 trials = 21 trials per condition, Fisher's exact test can detect a ~25 percentage-point difference at p < 0.05. Not enough for subtle differences, but sufficient to identify large improvements or regressions.
- Budget-constrained: at ~$2-5 per trial (Claude model), 63 trials is tractable.

### Model Control

All conditions use the **same model** across all trials:
- **Model:** `claude:opus` (as configured in `.wg/config.toml`)
- **Temperature:** 0.0 (deterministic — variance comes from model nondeterminism, not sampling)
- **Max turns:** 200 (safety valve; matches prior D/E experiments)
- **Wall-clock timeout:** 30 minutes per trial

Note: Prior TB experiments used `openrouter:minimax/minimax-m2.7`. This trial uses the wg's native executor model (`claude:opus`) because the enhanced executor features (decomp_guidance, separate-verify) are designed for the `claude` executor path, not the `native` executor. Using the same model removes model capability as a confound.

### Randomization

- Trial order is randomized within each condition to control for temporal effects (e.g., API latency, model serving load).
- Each condition runs all 7 tasks before moving to the next condition (block design) to avoid cross-condition contamination in shared git state.

### Environment Control

- Each trial runs in a **fresh worktree** (provided by `wg service start` agent isolation) for conditions B and C.
- Condition A trials run in an isolated temporary directory.
- All trials start from the same git commit (HEAD of main at trial start).
- No persistent state carries between trials.

---

## 4. Metrics

### Primary Metrics

| Metric | Measurement Method | Why It Matters |
|--------|-------------------|----------------|
| **Pass rate** | Binary: does the task's verification command (`--verify`) pass after the agent finishes? For condition A (no `wg done`), check if the task's acceptance criteria are met in the working tree. | The headline number. Does the enhanced executor produce more correct results? |
| **Time-to-completion** | Wall-clock seconds from agent spawn to `wg done` / agent exit. Measured by `wg show <task>` timestamps (conditions B, C) or process timing (condition A). | Faster is better if pass rate is equal. The enhanced executor's overhead (test discovery, separate verification) could slow agents down. |
| **Tool-call count** | Total tool invocations (Bash, Read, Edit, etc.) logged in the agent transcript. For conditions B/C, also count `wg` CLI invocations separately. | Proxy for computational cost. More tool calls = more tokens = more expensive. |

### Secondary Metrics

| Metric | Measurement Method | Why It Matters |
|--------|-------------------|----------------|
| **Subtask quality** (conditions B, C only) | For tasks that produce subtasks via `wg add`: (1) count of subtasks, (2) % of subtasks with `--after` edges, (3) % of subtasks with `--verify` gates, (4) correctness of dependency ordering. Measured by parsing `.wg/graph.jsonl` after the trial. | Tests whether adaptive decomposition produces better-structured subtask graphs than static guidance. |
| **Verification iterations** | Count of times the agent re-runs tests/checks before declaring done. Parsed from agent transcript (bash commands containing `test`, `pytest`, `cargo test`, `verify`). | Measures verification discipline. More iterations with convergence = good. Many iterations without convergence = thrashing. |
| **False-PASS rate** | Trials where the agent declared success (`wg done` or natural stop) but the task's verification failed. | Directly measures verification blind spot severity. Enhanced executor's separate-agent verification should reduce this. |
| **Failure diagnostics quality** | For failed trials: does the agent provide a reason? Is it actionable? Scored 0 (no info), 1 (vague), 2 (specific and actionable). Scored by human review. | Conditions B/C produce `wg fail --reason` or `wg log` entries. Condition A produces nothing. |
| **Token usage** | Total input + output tokens across all turns. From agent transcript metadata. | Cost metric. Enhanced features should not dramatically increase token usage. |

### Derived Metrics

| Metric | Formula |
|--------|---------|
| **Cost per pass** | Total tokens / number of passing trials |
| **Efficiency** | Pass rate / (avg tokens per trial) |
| **Decomposition overhead** | (Tool calls with wg - Tool calls without wg) / Tool calls without wg |
| **Verification effectiveness** | 1 - false_PASS_rate |

---

## 5. Baseline Configuration

### What "Old Executor" Means

The "old executor" (Condition B) is the wg executor with the five improvement tracks **disabled by config flags**. This approach is preferred over checking out an older git commit because:

1. It isolates exactly the features being tested (decomp, test discovery, verify mode)
2. All other executor improvements and bug fixes are shared across conditions
3. The same binary runs both conditions, eliminating build/version confounds

### Config Flags That Control Old vs. Enhanced Behavior

| Feature | Config Key | Old (Condition B) | Enhanced (Condition C) | Source File |
|---------|-----------|-------------------|----------------------|-------------|
| Adaptive decomposition | `coordinator.decomp_guidance` | `false` | `true` (default) | `src/config.rs:222-223` |
| Pre-task test discovery | `coordinator.auto_test_discovery` | `false` | `true` (default) | `src/config.rs:2227-2228` |
| Separate-agent verification | `coordinator.verify_mode` | `"inline"` | `"separate"` | `src/config.rs:2196-2197` |

### What Stays the Same Across B and C

- Full wg agent prompt (`REQUIRED_WORKFLOW_SECTION`, `GIT_HYGIENE_SECTION`, `MESSAGE_POLLING_SECTION`, `ETHOS_SECTION`, `GRAPH_PATTERNS_SECTION`)
- Agent isolation (worktrees)
- `wg` CLI availability and tool set
- Task descriptions, `--verify` commands, dependency edges
- Model (`claude:opus`), temperature (0.0), timeout (30 min)

### Verification of Baseline Config

Before running trials, verify the config takes effect:

```bash
# Condition B setup
wg config --coordinator-decomp-guidance false
wg config --coordinator-auto-test-discovery false
wg config --coordinator-verify-mode inline

# Confirm
wg config | grep -E 'decomp_guidance|auto_test_discovery|verify_mode'
# Expected: decomp_guidance = false, auto_test_discovery = false, verify_mode = inline

# Condition C setup
wg config --coordinator-decomp-guidance true
wg config --coordinator-auto-test-discovery true
wg config --coordinator-verify-mode separate

# Confirm
wg config | grep -E 'decomp_guidance|auto_test_discovery|verify_mode'
# Expected: decomp_guidance = true, auto_test_discovery = true, verify_mode = separate
```

---

## 6. Execution Plan

### Phase 1: Pre-Flight Checks (before any trials)

1. Verify `cargo build && cargo test` pass on current HEAD
2. Verify config flags toggle correctly (see Section 5)
3. Run one smoke trial per condition on `count-dataset-tokens` (a known-easy task) to confirm the harness works
4. Record the git commit hash as the trial baseline

### Phase 2: Condition A Trials (bare agent)

Run 7 tasks x 3 trials = 21 trials in an isolated environment:
- No wg initialization
- Agent receives task description only
- Manual verification of results against task acceptance criteria

### Phase 3: Condition B Trials (old executor)

1. Set config: `decomp_guidance=false`, `auto_test_discovery=false`, `verify_mode=inline`
2. For each task, create a wg task with `wg add` + appropriate `--verify`
3. Run `wg service start --max-agents 1` (single agent to avoid interference between trials)
4. Record results from `wg show` and agent transcripts

### Phase 4: Condition C Trials (enhanced executor)

1. Set config: `decomp_guidance=true`, `auto_test_discovery=true`, `verify_mode=separate`
2. Same tasks, same `--verify` commands, same model
3. Run `wg service start --max-agents 1`
4. Record results

### Phase 5: Analysis

1. Aggregate pass rates per condition per task
2. Compute all primary and secondary metrics
3. Run Fisher's exact test on aggregate pass rates (A vs B, A vs C, B vs C)
4. Per-task comparison to identify where enhanced executor helps vs. hurts
5. Examine subtask quality metrics (only C should show adaptive decomposition)
6. Review false-PASS rates (B's inline verify vs. C's separate-agent verify)

---

## 7. Expected Outcomes and Hypotheses

### Primary Hypothesis

**H1:** Condition C (enhanced executor) achieves a higher pass rate than Condition B (old executor) on the selected tasks.

Rationale: The enhanced executor addresses three documented failure modes:
- Self-verification blind spots (separate-agent verification)
- Counterproductive decomposition of atomic tasks (adaptive classification)
- Missing test awareness (test discovery + auto-verify)

### Secondary Hypotheses

**H2:** Condition C shows fewer false-PASS failures than Condition B.
Measurement: Count trials where agent declared success but verification failed.

**H3:** Condition C produces better-structured subtask graphs than Condition B.
Measurement: % of subtasks with `--after` edges and `--verify` gates.

**H4:** Condition C correctly avoids decomposing `cancel-async-tasks` and `regex-log` (classified as Atomic), while decomposing `build-cython-ext` and `merge-diff-arc-agi-task` (classified as Pipeline/FanOut).
Measurement: Subtask count per task, compared to task classification.

**H5:** Condition A (bare agent) may still be competitive with Condition B, based on prior pilot data showing A' ≥ D. The key question is whether Condition C's improvements push wg-assisted agents ahead of bare agents.

### Risk: Type II Error

With 21 trials per condition, we can only detect large effects (~25pp difference). If the enhanced executor provides a 10pp improvement, this trial will likely not reach statistical significance. The trial is designed for signal detection, not hypothesis confirmation. A follow-up with more trials would be needed to confirm subtle improvements.

---

## 8. Data Collection

### Per-Trial Data Points

```json
{
  "trial_id": "c-build-cython-ext-01",
  "condition": "C",
  "task_id": "build-cython-ext",
  "trial_number": 1,
  "model": "claude:opus",
  "temperature": 0.0,
  "git_commit": "<HEAD hash>",
  "config": {
    "decomp_guidance": true,
    "auto_test_discovery": true,
    "verify_mode": "separate"
  },
  "result": "pass|fail",
  "wall_clock_seconds": 185,
  "total_turns": 34,
  "tool_calls": 42,
  "wg_tool_calls": 7,
  "tokens_in": 450000,
  "tokens_out": 12000,
  "subtask_count": 3,
  "subtasks_with_after": 3,
  "subtasks_with_verify": 2,
  "verify_iterations": 4,
  "false_pass": false,
  "failure_diagnostic_quality": null,
  "termination_mode": "wg_done|wg_fail|no_tool_calls|timeout"
}
```

### Output Artifacts

- `terminal-bench/trials/results/condition-{a,b,c}/<task-id>-<trial>.json` — per-trial data
- `terminal-bench/trials/results/summary.json` — aggregated metrics
- `terminal-bench/trials/results/analysis.md` — comparison report with statistical tests

---

## Appendix A: Full Task Descriptions

### build-cython-ext
Build a Cython extension for pyknotid library; tests numpy version, repo clone, and importability. Multi-step: clone repo, patch for numpy compat, compile Cython extension, run test suite. Prior failure mode: D's self-verification passed basic imports but missed `test_ccomplexity`.

### cancel-async-tasks
Implement async task runner with cancellation and max-concurrency constraints. Single function, but `test_tasks_cancel_above_max_concurrent` catches a subtle edge case in cancellation under max-concurrency. Prior failure mode: All conditions' self-verification missed this edge case.

### overfull-hbox
Fix LaTeX overfull hbox warnings without modifying synonym definitions; compilation must succeed clean. Debugging task with constrained solution space. Prior failure mode: D had 0 verification iterations — couldn't determine what to verify for LaTeX.

### regex-log
Parse log files using regex to extract structured data. Atomic text-processing task. Prior failure mode: E decomposed into 5 subtasks, losing holistic regex context; 1 trial timed out at 3M tokens.

### custom-memory-heap-crash
Debug a custom memory heap crash without modifying protected files; must compile in debug and release modes. Multi-step C debugging requiring iterative compilation and testing.

### merge-diff-arc-agi-task
Initialize git repo, fetch two ARC-AGI bundles, and write an algorithm to solve merge-diff tasks. Multi-step reasoning task. E scored 100% thanks to decomposition; A' and D scored 67%.

### nginx-request-logging
Install and configure nginx with request logging and custom index page. System configuration with clear verification criteria. D scored 100% thanks to verification loop.

## Appendix B: Prior Experiment Cross-Reference

| Task | Condition A (50-turn cap) | A' (no cap) | D (self-verify) | E (org-gen) |
|------|--------------------------|-------------|-----------------|-------------|
| build-cython-ext | 33% | 100% | 33% | 100% |
| cancel-async-tasks | 33% | 67% | 67% | 0% |
| overfull-hbox | 33% | 33% | 33% | 67% |
| regex-log | 33% | 67% | 67% | 0% |
| custom-memory-heap-crash | 67% | 100% | 100% | 100% |
| merge-diff-arc-agi-task | 67% | 67% | 67% | 100% |
| nginx-request-logging | 33% | 67% | 100% | 100% |

Source: `terminal-bench/analysis/pilot-comparison.md`, `terminal-bench/analysis/pilot-tasks.json`
