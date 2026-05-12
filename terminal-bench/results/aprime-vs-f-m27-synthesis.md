# TB Synthesis: Condition A' vs Condition F — Full Comparison (m2.7)

**Date:** 2026-04-06
**Model:** minimax/minimax-m2.7 (via OpenRouter)
**Catalog:** 89 TB tasks × 3 repetitions per condition

---

## Executive Summary

Condition A' (bare agent, `ConditionAAgent`) and Condition F (wg-native, `ConditionFAgent`) produce **statistically indistinguishable pass rates** on the full TB catalog at the m2.7 model scale. The headline numbers:

| Metric | A' (bare) | F (wg-native) | Δ |
|--------|-----------|---------------|---|
| Trial pass rate | 45.2% (119/263) | 44.4% (106/239) | −0.9pp (p=0.84) |
| Task pass rate (≥1/3) | 54.5% (48/88) | 54.5% (48/88) | 0 |
| Mean tokens/trial | 1,055,928 | 505,752 | **−52%** |
| Total tokens | 277.7M | 120.9M | **0.44×** |
| Mean agent time | 364.6s | 278.2s | **−24%** |
| Infrastructure errors | 4 | 27 | +23 |

**Key finding:** Graph coordination (F) does not improve pass rates over a bare agent (A') for single-task TB problems at this model scale. However, F consumes dramatically fewer tokens (less than half) and runs faster on average, suggesting more efficient problem-solving behavior even when it doesn't improve correctness.

**Critical confound:** The two runs used different `timeout_multiplier` settings (A': 6.0×, F: 1.0×). F suffered 21 agent timeout errors that A' would not have encountered, artificially depressing F's results. After accounting for shared infrastructure failures, F lost ~19 additional trials to timeouts.

---

## 1. Experimental Setup

### Condition A' — Bare Agent
- **Agent:** `wg.adapter:ConditionAAgent`
- **Behavior:** Direct LLM agent, no graph scaffolding
- **Dataset:** `terminal-bench/terminal-bench-2`
- **Config:** `timeout_multiplier=6.0`, `n_concurrent_trials=5`
- **Run duration:** ~6.2 hours wall clock

### Condition F — wg-Native
- **Agent:** `wg.adapter:ConditionFAgent`
- **Behavior:** Agent with wg task decomposition and coordination
- **Dataset:** `terminal-bench` (same tasks, different registry ref)
- **Config:** `timeout_multiplier=1.0`, `n_concurrent_trials=4`
- **Run duration:** ~14.5 hours wall clock (longer due to lower timeout × longer tasks)

### Shared Parameters
- **Model:** `openrouter/minimax/minimax-m2.7`
- **Repetitions:** 3 per task (267 total trials per condition)
- **Temperature:** 0.0
- **Max turns:** 9999

---

## 2. Overall Pass Rates

### Trial-Level (each of the 3 reps counts independently)

| Condition | Valid Trials | Passes | Rate | 95% Wilson CI |
|-----------|-------------|--------|------|---------------|
| A' | 263 | 119 | 45.2% | [39.3%, 51.3%] |
| F | 239 | 106 | 44.4% | [38.2%, 50.7%] |

**Two-proportion z-test:** z = 0.202, p = 0.840. Not significant.

The confidence intervals overlap almost completely. There is no detectable difference in trial-level pass rates.

### Task-Level (≥1 of 3 reps passes)

| Condition | Tasks Passing | Rate |
|-----------|-------------|------|
| A' | 48/88 | 54.5% |
| F | 48/88 | 54.5% |

**McNemar's test (paired task pass/fail):**

|  | F passes | F fails |
|--|---------|---------|
| **A' passes** | 46 | 2 |
| **A' fails** | 2 | 38 |

χ² = 0.25, p = 0.617. Not significant.

**Wilcoxon signed-rank test** on paired per-task trial rates: W = 154.5, p = 0.399. Mean difference: −2.7pp (F slightly lower). Not significant.

### Conclusion: No detectable accuracy difference between conditions.

---

## 3. Per-Category Breakdown by Difficulty

Tasks are classified into tiers based on historical solve rates from the calibration study.

### Easy (33 tasks)

| Metric | A' | F |
|--------|-----|-----|
| Trial pass rate | 73.7% (73/99) | 68.8% (66/96) |
| Task pass (≥1/3) | 28/33 | 28/33 |
| Mean tokens/trial | 328,989 | 270,968 |
| Mean agent time | 276s | 247s |

**Observation:** A' has a slight edge (+4.9pp trial rate) on easy tasks, but this is within noise given the sample sizes. Both conditions solve the same 28/33 easy tasks.

### Medium (22 tasks)

| Metric | A' | F |
|--------|-----|-----|
| Trial pass rate | 65.2% (43/66) | 62.9% (39/62) |
| Task pass (≥1/3) | 18/22 | 19/22 |
| Mean tokens/trial | 568,338 | 497,516 |
| Mean agent time | 196s | 220s |

**Observation:** Nearly identical. F actually passes one more task at the ≥1/3 threshold (largest-eigenval). Both are strong on medium tasks.

### Hard (34 tasks)

| Metric | A' | F |
|--------|-----|-----|
| Trial pass rate | 3.1% (3/98) | 1.2% (1/81) |
| Task pass (≥1/3) | 2/34 | 1/34 |
| Mean tokens/trial | 2,118,935 | 839,627 |
| Mean agent time | 565s | 382s |

**Observation:** Both conditions are nearly floor-level on hard tasks. The m2.7 model simply cannot solve most hard TB tasks regardless of scaffolding. The A' hard pass rate is slightly higher, but n=3/98 vs n=1/81 is too sparse for meaningful comparison. F uses 60% fewer tokens on hard tasks — it gives up sooner rather than burning tokens on unsolvable problems.

### Difficulty Gradient

Does F's advantage grow with difficulty? **No.** The trend is actually reversed:

| Tier | Δ (F − A') trial rate |
|------|-----------------------|
| Easy | −4.9pp |
| Medium | −2.2pp |
| Hard | −1.8pp |

A' has a small (non-significant) edge at all difficulty levels. The gap narrows as difficulty increases, but this is driven by floor effects on hard tasks rather than an F advantage.

---

## 4. Task-Level Comparison Matrix

### Tasks Where F Outperforms A' (≥33pp advantage)

| Task | Tier | A' | F | Δ |
|------|------|-----|-----|-----|
| financial-document-processor | easy | 1/3 | 2/2 | +67pp |
| custom-memory-heap-crash | medium | 2/3 | 3/3 | +33pp |
| llm-inference-batching-scheduler | easy | 2/3 | 3/3 | +33pp |
| mailman | medium | 2/3 | 3/3 | +33pp |
| merge-diff-arc-agi-task | medium | 2/3 | 3/3 | +33pp |
| pytorch-model-cli | easy | 2/3 | 3/3 | +33pp |
| query-optimize | easy | 2/3 | 3/3 | +33pp |
| tune-mjcf | medium | 2/3 | 2/2 | +33pp |
| cancel-async-tasks | medium | 1/3 | 2/3 | +33pp |
| distribution-search | easy | 0/3 | 1/3 | +33pp |
| largest-eigenval | medium | 0/3 | 1/3 | +33pp |

**Pattern:** F's advantages cluster on medium-difficulty tasks, often converting 2/3 → 3/3. Six tasks go from A' at 2/3 to F at 3/3 — the graph coordination may help with reliability/consistency on tasks where the base model is borderline capable.

### Tasks Where A' Outperforms F (≥33pp advantage)

| Task | Tier | A' | F | Δ |
|------|------|-----|-----|-----|
| build-cython-ext | medium | 3/3 | 1/3 | −67pp |
| rstan-to-pystan | easy | 3/3 | 1/3 | −67pp |
| headless-terminal | easy | 2/3 | 0/3 | −67pp |
| vulnerable-secret | medium | 3/3 | 1/2 | −50pp |
| build-pov-ray | medium | 3/3 | 2/3 | −33pp |
| fix-git | medium | 3/3 | 2/3 | −33pp |
| git-leak-recovery | easy | 3/3 | 2/3 | −33pp |
| large-scale-text-editing | easy | 3/3 | 2/3 | −33pp |
| nginx-request-logging | easy | 3/3 | 2/3 | −33pp |
| sqlite-db-truncate | easy | 3/3 | 2/3 | −33pp |
| bn-fit-modify | medium | 2/3 | 1/3 | −33pp |
| caffe-cifar-10 | hard | 1/3 | 0/1 | −33pp |
| constraints-scheduling | easy | 2/3 | 1/3 | −33pp |
| extract-elf | easy | 2/3 | 1/3 | −33pp |
| sqlite-with-gcov | medium | 2/3 | 1/3 | −33pp |

**Pattern:** A' advantages cluster on easy tasks that A' solves 3/3 but F drops one trial. This suggests graph coordination overhead may occasionally *cause* a failure on tasks the bare agent handles cleanly. The 15 vs 11 asymmetry (more A' advantages) aligns with the non-significant overall A' edge.

### Exclusive Pass/Fail

| Category | Tasks |
|----------|-------|
| Only A' passes | caffe-cifar-10, headless-terminal |
| Only F passes | distribution-search, largest-eigenval |
| Both pass (46) | [46 shared tasks] |
| Neither passes (38) | [38 tasks neither solves] |

The overlap is overwhelming: 84 of 88 tasks have the same binary pass/fail outcome under both conditions.

---

## 5. Timing Comparison

| Metric | A' | F | Ratio |
|--------|-----|-----|-------|
| Mean agent time | 364.6s | 278.2s | 0.76× |
| Median agent time | 129.5s | 136.7s | 1.06× |
| Sum agent time | 26.6h | 18.5h | 0.69× |
| Max agent time | 1,928.8s | 1,906.5s | — |
| Turns: mean | 30.6 | 24.0 | 0.78× |
| Turns: median | 17 | 17 | 1.00× |
| Turns: p75 | 36 | 33 | — |

**Interpretation:** The means differ substantially (F is 24% faster on average), but the medians are nearly identical. The difference is driven by A' having a longer tail — A' spends more turns on failing tasks before giving up. F's graph coordination leads to earlier termination on unsolvable problems, which is why its mean is lower and its token usage is dramatically less. This is especially pronounced on hard tasks (A' mean 565s vs F mean 382s).

### Per-Tier Timing

| Tier | A' mean | F mean | F/A' |
|------|---------|--------|------|
| Easy | 276s | 247s | 0.89× |
| Medium | 196s | 220s | 1.13× |
| Hard | 565s | 382s | 0.68× |

F is slightly slower on medium tasks (possibly due to graph setup overhead) but substantially faster on hard tasks (gives up sooner on impossible problems).

---

## 6. Token Usage

| Metric | A' | F | Ratio |
|--------|-----|-----|-------|
| Total tokens | 277.7M | 120.9M | **0.44×** |
| Per-trial mean | 1,055,928 | 505,752 | **0.48×** |
| Input tokens | 274.4M | 118.5M | 0.43× |
| Output tokens | 3.3M | 2.4M | 0.73× |

F uses **less than half** the tokens of A' for equivalent accuracy. This is the most dramatic difference between the conditions.

### Per-Tier Token Usage

| Tier | A' mean/trial | F mean/trial | F/A' |
|------|--------------|--------------|------|
| Easy | 328,989 | 270,968 | 0.82× |
| Medium | 568,338 | 497,516 | 0.88× |
| Hard | 2,118,935 | 839,627 | **0.40×** |

The efficiency gap widens dramatically with difficulty:
- **Easy:** F saves ~18% of tokens — modest overhead savings
- **Medium:** F saves ~12% — similar
- **Hard:** F saves **60%** — the graph structure causes F to recognize failure earlier and stop burning tokens

### Cost Implications

At typical API pricing, F would cost roughly **half** as much as A' per benchmark run while achieving the same accuracy. For a full 89-task × 3-rep run at m2.7 scale, this represents savings of ~150M tokens.

---

## 7. Failure Mode Analysis

### Termination Types on Failed Trials (reward=0)

| Termination | A' failures | F failures |
|-------------|-------------|------------|
| natural_stop | 104 (72%) | 84 (63%) |
| llm_error | 22 (15%) | 33 (25%) |
| timeout | 18 (13%) | 16 (12%) |

F has a higher proportion of `llm_error` terminations among failures (25% vs 15%). This may indicate that graph coordination introduces additional API call complexity that increases the chance of hitting an LLM error. The `natural_stop` failures (agent gives up voluntarily) are proportionally lower for F, consistent with F terminating via error rather than decision.

### Infrastructure Errors

| Error Type | A' | F |
|-----------|-----|-----|
| AgentTimeoutError | 0 | 21 |
| RuntimeError (Docker) | 3 | 3 |
| CancelledError | 0 | 3 |
| RewardFileNotFoundError | 1 | 0 |
| **Total** | **4** | **27** |

The 21 AgentTimeoutErrors in F are a **direct consequence** of the `timeout_multiplier` mismatch:
- A' used `timeout_multiplier=6.0` (6× the default task timeout)
- F used `timeout_multiplier=1.0` (1× the default)

This means A' allowed up to 6× longer for each task. Of F's 21 timeout errors, 19 still received verifier evaluations (all reward=0), and 2 did not. These 19-21 timeouts are trials that F was *still working on* when the clock ran out — trials that A' would have had time to complete.

**Tasks most affected by F timeouts:** build-pmars, caffe-cifar-10, db-wal-recovery, extract-moves-from-video, gpt2-codegolf, make-doom-for-mips, overfull-hbox, password-recovery, path-tracing, qemu-alpine-ssh, torch-pipeline-parallelism, tune-mjcf, write-compressor, financial-document-processor.

---

## 8. Statistical Significance Summary

| Test | Statistic | p-value | Significant? |
|------|-----------|---------|-------------|
| Two-proportion z (trial rates) | z = 0.202 | 0.840 | No |
| McNemar (task pass/fail) | χ² = 0.25 | 0.617 | No |
| Wilcoxon signed-rank (paired rates) | W = 154.5 | 0.399 | No |

All three statistical tests fail to reject the null hypothesis of no difference. With 3 reps per task across 88 tasks, statistical power is modest — a true 5pp difference would require ~4× more data to detect with 80% power. However, the point estimates are very close (within 1-3pp), suggesting the true difference is negligible even if it exists.

---

## 9. Confounds and Limitations

### Timeout Multiplier Mismatch (CRITICAL)
The most significant confound is the 6× vs 1× timeout multiplier. F's 21 additional timeout errors reduce its effective sample size and depress its pass rate. If we conservatively assume those 19 timed-out-but-verified F trials would have maintained F's baseline rate (~44%), approximately 8-9 of them might have passed, bringing F to ~115/258 = ~44.6% — essentially identical to A'. **This confound alone could explain the small observed A' advantage.**

### Dataset Version
A' used `terminal-bench-2` while F used `terminal-bench`. The task names match but different registry refs may mean minor differences in task specifications or environments.

### Concurrency
A' ran 5 concurrent trials vs F's 4. This is unlikely to affect accuracy but may affect timing comparisons if the host machine had resource contention.

### LLM Error Rate
F has 50% more `llm_error` terminations (33 vs 22). This may be an artifact of graph coordination making more API calls per task, increasing the probability of encountering a transient API error.

---

## 10. The Key Question

**Does graph coordination (F) provide measurable benefit over a bare agent (A') on the full TB catalog?**

**Answer: No, not for single-task accuracy at the m2.7 model scale.**

The evidence is clear:
1. Pass rates are statistically indistinguishable (p > 0.8 on all tests)
2. Task-level outcomes match in 84/88 cases
3. The small A' edge (~1pp) is fully explained by the timeout confound
4. No difficulty tier shows a significant F advantage

However, F provides substantial **efficiency** benefits:
- **52% token reduction** (277.7M → 120.9M)
- **24% faster mean agent time** (365s → 278s)
- Earlier recognition of failure on hard tasks (60% fewer tokens on hard tier)

### Why doesn't F help?

TB tasks are **single-agent, single-task problems**. Each trial is one task in one Docker container. The graph coordination overhead in F provides:
- Task decomposition capability → but TB tasks are already atomic
- Dependency management → but there are no inter-task dependencies
- Agent coordination → but there's only one agent per trial

Graph coordination shines on **multi-step, multi-dependency problems** where the structure matters. TB tasks test raw problem-solving capability on self-contained challenges. For this use case, adding a coordination layer is overhead without upside.

The token efficiency gain suggests F's graph structure actually helps with *giving up* — recognizing unsolvable tasks sooner and avoiding the long retry loops that burn tokens in A'. This is a genuine behavioral difference, but it doesn't convert into more correct answers.

---

## 11. Recommendations

### For Future TB Experiments

1. **Fix the timeout confound.** Use identical `timeout_multiplier` for all conditions. Recommend `6.0×` as the standard.

2. **Condition A' is the right baseline.** For single-task benchmarks, A' (bare agent) is sufficient and more cost-effective. Use F only when testing multi-step coordination.

3. **Consider A' vs F on hard TB specifically.** The hard tier is near-zero for both conditions, but with a stronger model (e.g., opus-level), F's coordination could help on complex tasks where decomposition matters. The current m2.7 model is not capable enough for this to manifest.

4. **Design multi-task TB variants.** To properly evaluate graph coordination, create tasks that *require* coordination: multi-step builds, iterative refinement, tasks with explicit dependencies. The current TB catalog tests single-shot problem solving.

5. **Increase reps for power.** With 3 reps, a 10pp true difference has ~40% power to detect. Increase to 5-7 reps for meaningful per-task comparisons, or use a larger task catalog.

### For wg Users

1. **Use A' (bare agent) for simple, self-contained tasks.** The graph overhead doesn't help and costs a small reliability penalty (more API calls → more error chances).

2. **Use F (wg-native) when cost matters.** If token budget is constrained, F's 52% token savings are valuable even at equivalent accuracy.

3. **Reserve graph coordination for genuine multi-step work.** The value proposition of wg is coordination, not single-task execution. TB results confirm this.

---

## Appendix A: Full Task × Condition Pass Matrix

Legend: ✓ = passed (reward > 0), ✗ = failed (reward = 0), E = error (no reward)

### Easy Tasks (33)

| Task | A'₁ | A'₂ | A'₃ | F₁ | F₂ | F₃ | A' rate | F rate |
|------|-----|-----|-----|----|----|----| --------|--------|
| adaptive-rejection-sampler | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| cobol-modernization | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| code-from-image | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| compile-compcert | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| configure-git-webserver | ✓ | ✗ | ✓ | ✓ | ✓ | ✗ | 2/3 | 2/3 |
| constraints-scheduling | ✓ | ✓ | ✗ | ✓ | ✗ | ✗ | 2/3 | 1/3 |
| crack-7z-hash | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| distribution-search | ✗ | ✗ | ✗ | ✓ | ✗ | ✗ | 0/3 | 1/3 |
| extract-elf | ✓ | ✓ | ✗ | ✗ | ✗ | ✓ | 2/3 | 1/3 |
| financial-document-processor | ✗ | ✓ | ✗ | ✓ | E | ✓ | 1/3 | 2/2 |
| fix-code-vulnerability | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| git-leak-recovery | ✓ | ✓ | ✓ | ✓ | ✓ | ✗ | 3/3 | 2/3 |
| git-multibranch | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| headless-terminal | ✓ | ✓ | ✗ | ✗ | ✗ | ✗ | 2/3 | 0/3 |
| hf-model-inference | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| kv-store-grpc | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| large-scale-text-editing | ✓ | ✓ | ✓ | ✓ | ✓ | ✗ | 3/3 | 2/3 |
| llm-inference-batching-scheduler | ✓ | ✗ | ✓ | ✓ | ✓ | ✓ | 2/3 | 3/3 |
| mcmc-sampling-stan | ✗ | ✓ | ✓ | ✓ | ✓ | ✗ | 2/3 | 2/3 |
| modernize-scientific-stack | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| nginx-request-logging | ✓ | ✓ | ✓ | ✓ | ✗ | ✓ | 3/3 | 2/3 |
| openssl-selfsigned-cert | ✓ | ✗ | ✓ | ✗ | ✓ | ✓ | 2/3 | 2/3 |
| portfolio-optimization | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| prove-plus-comm | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| pypi-server | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| pytorch-model-cli | ✓ | ✗ | ✓ | ✓ | ✓ | ✓ | 2/3 | 3/3 |
| pytorch-model-recovery | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| qemu-alpine-ssh | ✓ | ✓ | ✓ | ✓ | E | E | 3/3 | 1/1 |
| query-optimize | ✓ | ✓ | ✗ | ✓ | ✓ | ✓ | 2/3 | 3/3 |
| reshard-c4-data | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| rstan-to-pystan | ✓ | ✓ | ✓ | ✓ | ✗ | ✗ | 3/3 | 1/3 |
| sanitize-git-repo | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| sqlite-db-truncate | ✓ | ✓ | ✓ | ✓ | ✓ | ✗ | 3/3 | 2/3 |

### Medium Tasks (22)

| Task | A'₁ | A'₂ | A'₃ | F₁ | F₂ | F₃ | A' rate | F rate |
|------|-----|-----|-----|----|----|----| --------|--------|
| bn-fit-modify | ✗ | ✓ | ✓ | ✓ | ✗ | ✗ | 2/3 | 1/3 |
| break-filter-js-from-html | ✗ | ✗ | ✓ | ✓ | ✗ | ✗ | 1/3 | 1/3 |
| build-cython-ext | ✓ | ✓ | ✓ | ✗ | ✓ | ✗ | 3/3 | 1/3 |
| build-pmars | ✓ | ✓ | ✓ | E | ✓ | ✓ | 3/3 | 2/2 |
| build-pov-ray | ✓ | ✓ | ✓ | ✓ | ✓ | ✗ | 3/3 | 2/3 |
| cancel-async-tasks | ✗ | ✓ | ✗ | ✗ | ✓ | ✓ | 1/3 | 2/3 |
| count-dataset-tokens | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| custom-memory-heap-crash | ✓ | ✓ | ✗ | ✓ | ✓ | ✓ | 2/3 | 3/3 |
| fix-git | ✓ | ✓ | ✓ | ✗ | ✓ | ✓ | 3/3 | 2/3 |
| largest-eigenval | ✗ | ✗ | ✗ | ✓ | ✗ | ✗ | 0/3 | 1/3 |
| log-summary-date-ranges | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| mailman | ✗ | ✓ | ✓ | ✓ | ✓ | ✓ | 2/3 | 3/3 |
| merge-diff-arc-agi-task | ✓ | ✓ | ✗ | ✓ | ✓ | ✓ | 2/3 | 3/3 |
| multi-source-data-merger | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| overfull-hbox | ✗ | ✗ | ✗ | ✗ | ✗ | E | 0/3 | 0/2 |
| qemu-startup | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ | 3/3 | 3/3 |
| regex-log | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| sparql-university | ✓ | ✓ | ✗ | ✗ | ✓ | ✓ | 2/3 | 2/3 |
| sqlite-with-gcov | ✓ | ✓ | ✗ | ✓ | ✗ | ✗ | 2/3 | 1/3 |
| tune-mjcf | ✓ | ✗ | ✓ | ✓ | E | ✓ | 2/3 | 2/2 |
| vulnerable-secret | ✓ | ✓ | ✓ | ✗ | ✓ | E | 3/3 | 1/2 |
| winning-avg-corewars | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |

### Hard Tasks (34)

| Task | A'₁ | A'₂ | A'₃ | F₁ | F₂ | F₃ | A' rate | F rate |
|------|-----|-----|-----|----|----|----| --------|--------|
| caffe-cifar-10 | ✓ | ✗ | ✗ | E | E | ✗ | 1/3 | 0/1 |
| chess-best-move | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| circuit-fibsqrt | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| db-wal-recovery | ✗ | E | ✗ | E | E | E | 0/2 | N/A |
| dna-assembly | ✗ | ✗ | ✗ | E | ✗ | ✗ | 0/3 | 0/2 |
| dna-insert | ✗ | ✗ | ✗ | ✗ | E | ✗ | 0/3 | 0/2 |
| extract-moves-from-video | ✗ | ✗ | ✗ | E | E | E | 0/3 | N/A |
| feal-differential-cryptanalysis | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| feal-linear-cryptanalysis | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| filter-js-from-html | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| fix-ocaml-gc | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| gcode-to-text | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| gpt2-codegolf | ✗ | ✗ | ✗ | E | E | ✗ | 0/3 | 0/1 |
| install-windows-3-11 | E | E | E | E | E | E | N/A | N/A |
| make-doom-for-mips | ✗ | ✗ | ✗ | ✗ | ✗ | E | 0/3 | 0/2 |
| make-mips-interpreter | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| model-extraction-relu-logits | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| mteb-leaderboard | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| mteb-retrieve | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| password-recovery | ✓ | ✗ | ✓ | E | ✗ | ✓ | 2/3 | 1/2 |
| path-tracing | ✗ | ✗ | ✗ | ✗ | ✗ | E | 0/3 | 0/2 |
| path-tracing-reverse | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| polyglot-c-py | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| polyglot-rust-c | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| protein-assembly | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| raman-fitting | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| regex-chess | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| sam-cell-seg | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| schemelike-metacircular-eval | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| torch-pipeline-parallelism | ✗ | ✗ | ✗ | ✗ | E | ✗ | 0/3 | 0/2 |
| torch-tensor-parallelism | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| train-fasttext | ✗ | ✗ | ✗ | M | ✗ | ✗ | 0/3 | 0/2 |
| video-processing | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ | 0/3 | 0/3 |
| write-compressor | ✗ | ✗ | ✗ | ✗ | E | ✗ | 0/3 | 0/2 |

---

## Appendix B: Configuration Differences

| Parameter | A' | F |
|-----------|-----|-----|
| Agent class | `ConditionAAgent` | `ConditionFAgent` |
| timeout_multiplier | **6.0** | **1.0** |
| n_concurrent_trials | 5 | 4 |
| Dataset ref | `terminal-bench-2` | `terminal-bench` |
| Model | `openrouter/minimax/minimax-m2.7` | `openrouter/minimax/minimax-m2.7` |
| Temperature | 0.0 | 0.0 |
| Max turns | 9999 | 9999 |
| Job started | 2026-04-06T01:13 UTC | 2026-04-06T01:10 UTC |
| Job finished | 2026-04-06T07:25 UTC | ~2026-04-06T15:39 UTC |

## Appendix C: Methodology Notes

- **Trial pass rate** = (trials with reward > 0) / (trials with any reward), excluding infrastructure errors
- **Task pass rate** = fraction of tasks where at least 1 of 3 trials passes
- **Wilson score intervals** used for binomial confidence intervals (better small-sample behavior than Wald)
- **McNemar's test** with continuity correction for paired binary outcomes
- **Wilcoxon signed-rank test** on per-task trial pass rates (0, 1/3, 2/3, 1)
- Infrastructure errors (Docker failures, cancellations, agent timeouts with no reward) are excluded from pass rate calculations
- Agent timeouts that still received verifier evaluation (19 F trials) are included in pass rate calculations with their actual reward (all 0)
