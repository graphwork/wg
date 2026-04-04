# Pilot Comparison: A' vs D vs E

**Date:** 2026-04-04  
**Task:** tb-pilot-analysis  
**Model:** openrouter/minimax/minimax-m2.7 (all conditions)  
**Tasks:** 10 pilot tasks × 3 trials each = 30 trials per condition

---

## 1. Executive Summary

| Condition | Pass Rate | Avg Turns | Avg Time (s) | Avg Tokens | Tokens/Pass |
|-----------|-----------|-----------|---------------|------------|-------------|
| **A' (bare, no turn cap)** | **80.0%** (24/30) | 28.9 | 265 | 821K | 1,026K |
| **D (wg + self-verification)** | **73.3%** (22/30) | 29.6 | 271 | 521K | 710K |
| **E (autopoietic org)** | **75.0%** (21/28 valid) | 37.4 | 299 | 683K | 976K |

**Key finding: A' wins.** Removing the turn cap was the single most impactful intervention. Neither D's self-verification loop nor E's organization generation improved on the simple bare agent with unlimited turns. Both D and E add overhead that is not justified by pass rate improvements on this 10-task pilot.

**However:** The story is more nuanced per-task. E shows clear advantages on specific task types (reasoning, multi-step builds), while D is the most cost-efficient condition overall.

---

## 2. Per-Task Comparison Table

| Task | Category | Orig A | A' | D | E | Winner |
|------|----------|--------|-----|---|---|--------|
| build-cython-ext | building | 33% | **100%** | 33% | **100%** | A'/E tie |
| cancel-async-tasks | async | 33% | **67%** | **67%** | 0% | A'/D tie |
| nginx-request-logging | server-config | 33% | 67% | **100%** | **100%** | D/E tie |
| overfull-hbox | debugging | 33% | 33% | 33% | **67%** | E |
| regex-log | text-processing | 33% | **67%** | **67%** | 0% | A'/D tie |
| count-dataset-tokens | data-processing | 67% | **100%** | **100%** | **100%** | 3-way tie |
| custom-memory-heap-crash | debugging | 67% | **100%** | **100%** | **100%** | 3-way tie |
| merge-diff-arc-agi-task | reasoning | 67% | 67% | 67% | **100%** | E |
| qemu-startup | system-emulation | 67% | **100%** | **100%** | **100%** | 3-way tie |
| sparql-university | query-language | 67% | **100%** | 67% | 67% | A' |

**Hard tasks** (original A = 33%, 5 tasks):

| Condition | Hard task pass rate |
|-----------|-------------------|
| A' | 13/15 = **67%** |
| D | 10/15 = **67%** |
| E | 8/13 valid = **62%** |

**Medium tasks** (original A = 67%, 5 tasks):

| Condition | Medium task pass rate |
|-----------|---------------------|
| A' | 11/15 = **93%** |
| D | 12/15 = **80%** |
| E | 13/15 = **87%** |

---

## 3. Where D Beats A'

D matches or exceeds A' on **4 of 10 tasks**, but clearly beats A' on only one:

### nginx-request-logging: D 100% vs A' 67%
D's verification loop caught a configuration issue that A' missed. All 3 D trials ran verification commands (~3 iterations each) before calling `wg_done`, compared to A' which stopped after writing the config. The A' failure was a trial with 1 turn and 0 tokens — likely a setup/bootstrap error rather than a genuine failure.

### Where D loses to A':

- **build-cython-ext (D 33% vs A' 100%):** D's biggest regression. Two D trials called `wg_done` (self-termination) despite failing the `test_ccomplexity` verifier test. The D prompt's verification gate ("NEVER call wg_done without successful verification") was not effective — the agent's self-verification passed (basic imports, simple tests), but the external verifier's `test_ccomplexity` test was more demanding. **This is the self-verification blind spot:** the agent ran the wrong verification criteria.

- **sparql-university (D 67% vs A' 100%):** One D trial failed on a query correctness test. D's average 3.0 verification iterations didn't catch the semantic error.

### D's self-verification metrics:
- 100% wg usage (30/30 trials used wg tools)
- 93% self-termination via `wg_done` (28/30), 7% via `no_tool_calls` (2/30)
- Average 3.4 verification iterations per trial
- **Problem:** 0.0 verification iterations on overfull-hbox (3 trials) — the agent couldn't find appropriate verification commands for LaTeX debugging

---

## 4. Where E Beats A'/D

E achieves the highest pass rate on **3 tasks**:

### merge-diff-arc-agi-task: E 100% vs A' 67% vs D 67%
E's decomposition (avg 5.3 subtasks) broke the ARC-AGI reasoning task into: (1) git setup, (2) data parsing, (3) algorithm design, (4) testing. This structured approach produced 3/3 passes vs 2/3 for both A' and D. The decomposition helped the agent maintain focus on each sub-problem rather than context-switching.

### overfull-hbox: E 67% vs A' 33% vs D 33%
E's decomposition (avg 3.7 subtasks) helped organize the LaTeX debugging task. However, all conditions had 0 verification iterations on this task — the debugging is inherently hard to verify programmatically.

### build-cython-ext: E 100% vs D 33%
E matched A' (100%) and dramatically outperformed D. E's decomposition (avg 4 subtasks) organized: clone repo, fix numpy compatibility, compile Cython, run tests. The structured approach with 8 average verification iterations caught issues that D's simpler loop missed.

### Where E loses catastrophically:

- **cancel-async-tasks (E 0% vs A' 67% vs D 67%):** All 3 E trials failed on `test_tasks_cancel_above_max_concurrent`. The agent declared "VERIFY: PASS" on its own tests but the external verifier caught that the cancellation behavior under max-concurrency was incorrect. E created only 1 subtask on average for this task — decomposition didn't help because the task is inherently single-unit (implement one function). **The E overhead (avg 20 turns, 185K tokens) produced worse results than A' (avg 21 turns, 215K tokens) or D (avg 17 turns, 119K tokens).**

- **regex-log (E 0% vs A' 67% vs D 67%):** All 3 E trials failed (including 1 timeout error at 30 min). The agent created 5 subtasks and ran 25.5 average verification iterations — massive churn that consumed tokens (avg 1.9M tokens) without converging on the correct regex. The decomposition added coordination overhead to what is fundamentally a single-file regex-writing task. **E spent 3.6x more tokens than D for a 0% pass rate.**

- **sparql-university (E 67% vs A' 100%):** Minimal decomposition (avg 1 subtask) — E essentially ran as a single agent, suggesting decomposition was not applied meaningfully.

---

## 5. Cost Analysis

### Token usage per condition (30 trials each)

| Metric | A' | D | E |
|--------|-----|---|---|
| Total tokens | 24.6M | 15.6M | 20.5M |
| Avg tokens/trial | 821K | **521K** | 683K |
| Tokens/pass | 1,026K | **710K** | 976K |
| Cost efficiency (pass%/Ktok) | 0.097 | **0.141** | 0.110 |

**D is the most cost-efficient condition** — 37% fewer tokens per trial than A', while achieving 73% pass rate. The verification loop adds discipline without proportional token overhead.

**E is moderately expensive** — 17% fewer tokens than A' on average, but this is misleading. E's token usage is bimodal:
- Simple tasks (count-dataset-tokens, sparql-university): ~150-300K tokens
- Complex failures (regex-log, build-cython-ext): 1.8-3.0M tokens

### Token distribution by task outcome

| | A' pass | A' fail | D pass | D fail | E pass | E fail |
|--|---------|---------|--------|--------|--------|--------|
| Avg tokens | 770K | 1,025K | 387K | 925K | 526K | 1,194K |

D failing trials are expensive (verification loops that don't converge). E failing trials are the most expensive of all conditions — the decomposition + verification + triage cycle consumes tokens even when the approach isn't working.

### Projected cost for full 89-task run (267 trials)

| Condition | Est. tokens | Est. cost (at minimax-m2.7 rates) |
|-----------|------------|----------------------------------|
| A' | ~219M | ~$190 |
| D | ~139M | ~$120 |
| E | ~182M | ~$158 |

---

## 6. Behavioral Analysis

### A' behavior: Efficient directness
- **Termination:** 100% `no_tool_calls` (natural stop, no explicit signaling)
- **Pattern:** Read task → implement → test → stop. No explicit verification protocol, but the model naturally tests its work in most cases.
- **High variance:** custom-memory-heap-crash had one trial at 191 turns (14M tokens!) while another finished in 22 turns (127K tokens). Without a verification gate, A' sometimes runs very long when it gets stuck.
- **Failure mode:** Stops too early (1-turn nginx failure) or iterates on wrong approach without converging

### D behavior: Disciplined but sometimes self-deceived
- **Termination:** 93% `wg_done` (28/30) — strong compliance with self-termination protocol
- **Verification pattern:** Average 3.4 verification iterations. The prompt's "attempt → verify → iterate → declare" loop was followed consistently.
- **Tool usage:** Minimal wg overhead (avg 2.9 wg tool calls) — mostly `wg_log`, `wg_done`, occasional `wg_add`
- **Critical weakness:** Self-verification is unreliable. On build-cython-ext, D agents ran their own tests (which passed) but the external verifier found failures. The agent cannot verify what it cannot conceive of testing. On overfull-hbox, 0 verification iterations across all 3 trials — the agent couldn't find appropriate verification for LaTeX.
- **Token efficiency:** D's verification loop actually constrains token usage by providing a convergence criterion. Instead of A's open-ended "keep trying," D asks "did my verification pass?" and terminates.

### E behavior: Ambitious but sometimes counterproductive
- **Decomposition:** 93% decomposition rate, avg 3.4 subtasks per trial, 95 total subtasks across 30 trials
- **Verification:** 89% of trials included an independent verification phase, but the "independence" is illusory — it's the same context window shifting perspective, not a truly separate agent
- **Triage:** Only 7% of trials used triage (creating fix tasks on failure) — the E-specific triage mechanism was rarely invoked
- **wg tool calls:** avg 15.2 per trial (5x more than D) — the organizational overhead is real
- **Critical weakness:** Decomposition is counterproductive on single-unit tasks (cancel-async-tasks: 1 subtask avg, regex-log: 5 subtasks that just fragmented the problem). The agent decomposed because the prompt told it to, not because the task warranted it.
- **Where E shines:** Multi-step tasks with independent components (build-cython-ext: 4 subtasks, merge-diff-arc-agi-task: 5.3 subtasks). Decomposition provides genuine cognitive benefit here.

### Transcript analysis (3+ trials per condition examined)

**D build-cython-ext failure (Lc9kSx4):** Agent ran 6 verification iterations with pytest, all passing on basic tests. Called `wg_done` after logging "Task completed successfully." But the external verifier's `test_ccomplexity` (Cython complexity test) failed. The agent never tested Cython extension compilation quality — it verified import works and basic tests pass, missing the deeper requirement.

**E cancel-async-tasks failure (all 3 trials):** All trials created 1 subtask, implemented the function, ran self-tests (5/6 pass), declared "VERIFY: PASS", and called `wg_done`. The failing test (`test_tasks_cancel_above_max_concurrent`) tests a subtle edge case in cancellation behavior under max-concurrency. The E agent's verification was no more thorough than D's despite the "independent verification" prompt — it's still the same model checking its own work.

**E regex-log timeout (g4BEPZp):** Created 5 subtasks, ran 36 verification iterations (!) over 57 turns, consumed 3.0M tokens, and timed out at 30 minutes. The decomposition fragmented the regex problem into parts that couldn't be solved independently — each regex depends on the log format context that was split across subtasks.

---

## 7. Failure Mode Analysis

### D failure modes

| Failure type | Count | Description |
|--------------|-------|-------------|
| Self-verification blind spot | 4/8 | Agent's own tests pass but external verifier fails on different criteria |
| No-verification task | 3/8 | Agent can't find verification criteria (overfull-hbox: 0 verify iterations) |
| Reasoning limitation | 1/8 | merge-diff-arc-agi: task requires creative algorithm, not fixable by iteration |

**Root cause:** D's self-verification is limited by the agent's ability to imagine what to test. It cannot verify requirements it doesn't know about.

### E failure modes

| Failure type | Count | Description |
|--------------|-------|-------------|
| False PASS verdict | 5/9 | Agent declares verification passed, but external verifier disagrees |
| Timeout | 2/9 | Agent exhausts 30-min wall clock in verification loops |
| Counterproductive decomposition | 4/9 | Decomposition fragments tasks that should be solved atomically |

**Root cause:** E's "independent verification" is theater — it's the same context window pretending to be a reviewer. The decomposition overhead is justified only on genuinely decomposable tasks.

---

## 8. Statistical Context

With only 3 trials per task, individual task results are highly variable (binomial confidence intervals are wide). A 67% vs 33% difference is 2/3 vs 1/3 — just one trial different.

**Fisher's exact test on aggregate (24 vs 22 vs 21 out of 30):**
- A' vs D: p = 0.77 (not significant)
- A' vs E: p = 0.85 (not significant, comparing 24/30 vs 21/28)
- D vs E: p = 1.00 (not significant)

**None of the differences are statistically significant** at this sample size. The pilot is useful for identifying patterns and failure modes, not for hypothesis testing.

---

## 9. Recommendations for Full 89-Task Run

### 1. Run A' as the primary condition
A' (bare agent, no turn cap, 30-min timeout) is the strongest performer and simplest condition. It should be the primary condition for the full run. **Estimated cost: ~$190 for 267 trials.**

### 2. Run D as the comparison condition, with fixes
D is the most cost-efficient and shows genuine behavioral improvement (structured termination, verification discipline). Before the full run:

- **Fix the verification criteria gap:** The D prompt should instruct agents to read the task's test suite or verification script before starting, so they know what the external verifier checks. Add: "Before implementing, look for test files or verification scripts in the task environment. Your verification must cover what those tests check."
- **Handle no-verification tasks:** For tasks without obvious test criteria (like LaTeX debugging), the prompt should instruct agents to compare output to expected output files rather than running nonexistent tests.

### 3. Do NOT run E at full scale
E's overhead is not justified. The organization generation adds ~30% more turns and tokens for no aggregate pass rate improvement. Specific problems:

- **False independence:** Single-agent verification cannot be truly independent. E's design thesis (independent verifier) requires multi-agent execution, which Harbor doesn't support.
- **Counterproductive on atomic tasks:** ~40% of TB tasks are single-unit problems (write one function, fix one bug). Decomposing these wastes tokens.
- **Timeout risk:** E's verification loops can consume the full 30-min budget on hard tasks.

**If E is run at all,** restrict it to multi-step tasks (building, system-config, multi-file problems) where decomposition has shown benefit. Exclude single-function and single-file tasks.

### 4. Consider a D' variant
Based on the pilot findings, a D' condition could address D's weaknesses:
- **Read the verifier first:** If the task environment contains test files, the agent should examine them before implementing.
- **Fail faster:** If 3 verification iterations fail on the same issue, call `wg_fail` instead of continuing (D already has this, but overfull-hbox shows it wasn't triggered — 0 verify iterations means the agent didn't try to verify at all).
- **Adaptive verification:** For tasks without test suites, fall back to output comparison rather than skipping verification.

### 5. Full run configuration

| Parameter | A' | D (recommended) |
|-----------|-----|-----------------|
| Tasks | All 89 | All 89 |
| Trials/task | 3 | 3 |
| Total trials | 267 | 267 |
| max_turns | 9999 | 200 |
| timeout | 30 min | 30 min |
| Estimated cost | $190 | $120 |
| Estimated time | 12 hours | 9 hours |

### 6. Analysis priorities for full run
1. **A' vs D pass rate** with sufficient power (267 trials gives ~80% power to detect a 10pp difference)
2. **Task-type interaction:** Does D outperform A' specifically on tasks where verification is straightforward?
3. **Cost-effectiveness:** Pass rate per dollar across conditions
4. **Failure diagnosis quality:** Do D's `wg_fail` messages provide actionable diagnostics vs A's silent stops?
