# Arena Evaluation Research Spec

Research spec for integrating FLIP-style backward-inference evaluation into wg.

## 1. Paper Summary

**Citation:** Wang, Y., Brahman, F., Feng, S., Xiao, T., Hajishirzi, H., & Tsvetkov, Y. (2025). *Small Reward Models via Backward Inference*. arXiv:2602.13551. Code: https://github.com/yikee/FLIP

**Problem:** LLM-as-a-Judge evaluation (the current `wg evaluate` approach) relies on the evaluator model having strong reasoning/judgment capabilities. Small models perform ~41% worse than large models at direct judgment (§1, p.2). This makes cheap, scalable evaluation unreliable.

**Method — FLIP (FLipped Inference for Prompt reconstruction):**
1. Given a response `y`, ask a model to infer the instruction `x'` that would produce it: `x' ~ p_φ(x'|y)`
2. Compute F1 (word-level precision/recall) between inferred instruction `x'` and actual instruction `x`
3. Use `r = F1(x, x')` as the quality score

**Key insight:** Generation is easier than judgment for small models ("validation-generation gap," §5, p.8). A model that can't reliably *judge* whether a response is good can still *generate* a plausible instruction from a good response — and instruction recovery is easier when the response faithfully followed the instruction.

**Key results:**
- Outperforms LLM-as-Judge by +99.4% avg on RewardBench2 across 13 small models (Table 1, §4.1)
- 12B FLIP matches 72.9–76% accuracy of large commercial LLM-as-Judge (Table 2)
- Performance gap *increases* as model size *decreases* — 75% improvement for 1B models
- Robust against adversarial attacks and reward hacking (§5, Figure 6)
- Effective for Best-of-N selection and GRPO RL training (§4.2–4.3)

## 2. Core Mechanism

### Inputs/Outputs

| Component | Value |
|-----------|-------|
| **Input** | Response `y` (and optionally user prompt for system-prompt inference) |
| **Model** | Any small LM (1B–12B tested) — no fine-tuning needed |
| **Prompt** | "Infer a single instruction that would most plausibly generate the given response" |
| **Output** | Inferred instruction `x'` |
| **Score** | `F1(x, x')` — word-level F1 between original and inferred instructions |

### Scoring Formula

```
Precision = |tokens(x) ∩ tokens(x')| / |tokens(x')|
Recall    = |tokens(x) ∩ tokens(x')| / |tokens(x)|
F1        = 2 · Precision · Recall / (Precision + Recall)
```

No learned parameters. No training. Just generation + string matching.

### Comparison Ranking

For Best-of-N (arena-style) selection among responses `{y₁, ..., yₙ}` to the same instruction:
- Score each independently: `rᵢ = F1(x, FLIP(yᵢ))`
- Rank by score: `y_π(1) ≻ y_π(2) ≻ ... ≻ y_π(n)`

### Why It Works

The paper hypothesizes the **validation-generation gap**: even GPT-4 has only 76% consistency between generating and validating answers (§5, p.8). Small models have a *larger* gap — they are relatively better at generation than judgment. FLIP exploits this by converting evaluation (hard for small models) into generation (easier).

### Limitations

- Responses that repeat the instruction verbatim inflate scores (rare, detectable — §7)
- Cross-language instruction/response pairs break F1; need LLM-judge fallback (§7)
- Longer responses yield better FLIP scores (§5, Figure 5) — may need normalization

## 3. Relevance to wg

### 3a. As a `wg evaluate` Component

**Current system** (`src/commands/evaluate.rs`): Spawns a Claude instance with an evaluator prompt, asks for JSON `{score, dimensions, notes}`. Uses LLM-as-Judge — the model reads the task definition, agent identity, artifacts, and logs, then judges quality on a 0–1 scale with dimensional breakdowns (correctness, completeness, efficiency, style_adherence).

**FLIP integration point:** FLIP could replace or supplement the LLM-as-Judge call at line 188 of `evaluate.rs`. Instead of (or in addition to) asking a large model to judge quality:
1. Take the task description as instruction `x`
2. Take agent output (log entries / artifact contents) as response `y`
3. Run FLIP with a small local model to get `F1(x, FLIP(y))`
4. Record as an evaluation with `source: "flip"` (line 245 currently uses `source: "llm"`)

**Benefit:** Cheaper, faster, more robust evaluation. Could run FLIP on every task automatically (currently evaluation is opt-in due to cost). The `Evaluation` struct (`src/agency.rs:204`) already supports `source` field differentiation.

**Files:** `src/commands/evaluate.rs`, `src/agency.rs` (EvaluatorInput, render_evaluator_prompt, Evaluation struct)

### 3b. As a Model Selection Mechanism

**Current system:** Model is configured statically via `wg config agent.model` or per-task `--model` flag. The coordinator (`src/commands/service.rs`) spawns agents with one fixed model.

**FLIP integration point:** Run the same task prompt through N different models, collect responses, score each with FLIP, pick the best:
1. For a given task, spawn N candidate completions with different models
2. FLIP-score each: `rᵢ = F1(task_description, FLIP(responseᵢ))`
3. Select the highest-scoring response
4. Record which model won (feed into evolution)

**Benefit:** Automated model selection per-task without expensive human or LLM-as-Judge evaluation. Especially useful when trying new models — run both old and new on the same task, pick winner objectively.

**Files:** `src/service/executor.rs` (TemplateVars, agent spawning), `src/commands/service.rs` (coordinator loop)

### 3c. As a Context Selection Mechanism

**Current system:** Agent identity (role + motivation) is resolved in `executor.rs:resolve_identity()` and injected into the agent prompt. The coordinator assigns agents based on skill matching and performance history.

**FLIP integration point:** Given a task, generate candidate prompts with different context configurations (different roles, different motivations, different preambles), run FLIP on each:
1. Construct N prompt variants (e.g., different role descriptions, different skill preambles)
2. For each variant, generate a response using a fixed model
3. FLIP-score each response against the original task description
4. Select the prompt/context configuration that yields the highest score

**Benefit:** Empirical prompt optimization. Rather than guessing which role or motivation works best for a task type, measure it. Could be run offline as a batch experiment.

**Files:** `src/service/executor.rs` (TemplateVars, resolve_identity, resolve_skills_preamble), `src/agency.rs` (render_identity_prompt)

### 3d. As an Agent Evolution Input

**Current system** (`src/commands/evolve.rs`): Loads all evaluations, builds a performance summary per role/motivation, sends to an LLM evolver that proposes mutations (create/modify/retire roles and motivations). Evolution strategies: mutation, crossover, gap-analysis, retirement, motivation-tuning.

**FLIP integration point:** FLIP scores could provide a cheaper, higher-volume signal for evolution:
1. Auto-evaluate every completed task with FLIP (cheap, no API call needed with local model)
2. Feed FLIP scores into the performance summary used by `build_performance_summary()` in `evolve.rs`
3. Use FLIP score distributions (not just averages) to identify underperforming roles
4. FLIP's adversarial robustness (§5) means evolution can trust the signal more than LLM-judge scores

**Benefit:** More evaluations → better evolution signal. Currently evaluations are sparse because they require an expensive LLM call. With FLIP running on every task, evolution gets dense performance data.

**Files:** `src/commands/evolve.rs` (build_performance_summary, EvolverOutput), `src/agency.rs` (PerformanceRecord, EvaluationRef, record_evaluation)

## 4. Subtask Decomposition

### Subtask 1: FLIP Evaluator for `wg evaluate`

**Scope:** Add a `--method flip` flag to `wg evaluate` that uses backward inference instead of LLM-as-Judge.

**Files to read:**
- `src/commands/evaluate.rs` — current evaluation pipeline
- `src/agency.rs:398` — `render_evaluator_prompt()` (reuse task data extraction)
- `src/agency.rs:204` — `Evaluation` struct (source field = "flip")

**Deliverable:** FLIP evaluation path in evaluate.rs that:
- Constructs a backward-inference prompt from task artifacts/logs
- Runs it through a configurable model (default: small/local)
- Computes F1 between task description and inferred instruction
- Records evaluation with `source: "flip"`

### Subtask 2: Arena-Style Model Selection

**Scope:** Add a `wg arena` command or `--arena` flag that runs N models on the same task and picks the best.

**Files to read:**
- `src/service/executor.rs` — how agents are spawned with different models
- `src/commands/service.rs` — coordinator loop, model config
- `src/config.rs` — model configuration

**Deliverable:** Design for a Best-of-N model comparison workflow:
- Task gets N candidate completions from different models
- Each scored via FLIP
- Winner gets recorded, loser responses discarded
- Model win-rates tracked for future selection

### Subtask 3: Context/Prompt Arena

**Scope:** Use FLIP to compare different prompt/context configurations for the same task.

**Files to read:**
- `src/service/executor.rs:36` — `TemplateVars::from_task()` and identity resolution
- `src/agency.rs` — `render_identity_prompt()`, role/motivation loading
- `src/commands/evolve.rs` — how roles and motivations are varied

**Deliverable:** Design for prompt variant testing:
- Generate N prompt variants (different roles, motivations, preambles)
- Run each through a fixed model
- FLIP-score and rank
- Output: which context configuration works best for which task types

### Subtask 4: FLIP-Powered Evolution Signal

**Scope:** Auto-run FLIP evaluation on every completed task and feed scores into evolution.

**Files to read:**
- `src/commands/evolve.rs:180` — `build_performance_summary()`
- `src/commands/service.rs` — coordinator post-task hooks (where auto-eval would trigger)
- `src/agency.rs` — `record_evaluation()`, `PerformanceRecord`

**Deliverable:** Design for continuous FLIP evaluation:
- Hook into task completion to auto-run FLIP
- Store with `source: "flip-auto"` to distinguish from manual evaluations
- Ensure evolution weighs FLIP scores appropriately (possibly lower weight than human/LLM-judge)
- Track FLIP score distributions, not just averages
