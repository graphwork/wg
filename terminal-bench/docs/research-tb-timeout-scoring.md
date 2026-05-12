# Research: Terminal-Bench Timeout and Scoring Rules for Leaderboard Submissions

**Task:** tb-research-timeout
**Date:** 2026-04-08
**Status:** Complete

---

## 1. Timeout Rules

### Per-task timeouts (defined in `task.toml`)

Each TB 2.0 task defines its own `[agent].timeout_sec` in its `task.toml` file. These are the **authoritative** timeouts that must be used for valid leaderboard submissions.

**Distribution across 89 tasks (from cached task.toml files):**

| Agent Timeout (seconds) | Count | Human-readable |
|------------------------|-------|----------------|
| 900 | 108* | 15 minutes |
| 1,200 | 12 | 20 minutes |
| 1,800 | 32 | 30 minutes |
| 2,400 | 4 | 40 minutes |
| 3,600 | 24 | 1 hour |
| 7,200 | 2 | 2 hours |
| 12,000 | 2 | 3.3 hours |

\* Count exceeds 89 because multiple dataset versions are cached; the distribution pattern holds.

**Range:** 15 minutes (easy tasks) to 3.3 hours (extreme tasks).

**By difficulty:**
- **Easy:** 750–900 seconds (12.5–15 min)
- **Medium:** 600–12,000 seconds (10 min–3.3 hours), most at 900s
- **Hard:** 900–7,200 seconds (15 min–2 hours)

### Timeout is NOT configurable by the submitter

The leaderboard validation bot enforces:
- `timeout_multiplier` **must equal 1.0** (checked in `config.json` of every trial)
- `override_timeout_sec` must be null (no agent timeout overrides)
- `max_timeout_sec` must be null
- No verifier timeout overrides
- No resource overrides (`override_cpus`, `override_memory_mb`, `override_storage_mb`)

**Source:** HuggingFace `harborframework/terminal-bench-2-leaderboard` submission validation rules; `harbor run --help` (the `--timeout-multiplier` flag defaults to 1.0).

### What happens when an agent times out

Harbor raises `AgentTimeoutError` when the agent exceeds its `task.toml` timeout (multiplied by `timeout_multiplier`, which must be 1.0). From the Harbor source (`harbor/trial/trial.py:291-295`):

```python
async with asyncio.timeout(self._agent_timeout_sec):
    await self._run_agent()
except asyncio.TimeoutError:
    raise AgentTimeoutError(
        f"Agent execution timed out after {self._agent_timeout_sec} seconds"
    )
```

**Critically, the verifier still runs after a timeout** (`trial.py:559-562`). The timeout is caught, logged as an exception, but verification proceeds on whatever partial work the agent completed. This means:

- **If partial work passes verification → reward = 1.0** (pass)
- **If partial work fails verification → reward = 0.0** (fail)
- **The timeout itself does NOT automatically score 0.** Partial credit is possible if the agent solved the task before running out of time.

In practice, most timeouts produce reward = 0.0 because agents that time out typically haven't completed the task. But the mechanism allows credit for partial success.

`AgentTimeoutError` is in the default `retry_exclude` list (`harbor/models/job/config.py:221`), meaning timed-out trials are NOT retried by default.

### Our compliance issue

Our `reproduce.sh` passes `--timeout 1800` to `harbor run`, which overrides per-task timeouts with a flat 30-minute cap. This:
- **Truncates hard/extreme tasks** that need up to 3.3 hours
- **May extend easy tasks** beyond their 15-minute limit (also non-compliant)
- **Is detected by the validation bot** (sets `override_timeout_sec` in config.json)

**Fix:** Remove `--timeout "$TIMEOUT"` from `reproduce.sh` to use per-task defaults.

**Source:** `terminal-bench/reproduce.sh:29,116`; `terminal-bench/analysis/tb2-runtime-compliance-audit.md`

---

## 2. Scoring

### Binary pass/fail (reward = 0.0 or 1.0)

Scoring is **binary**. Each trial produces a single `reward` value in `result.json`:

```json
"verifier_result": {
    "rewards": {
        "reward": 1.0   // or 0.0
    }
}
```

Confirmed by examining all 89 result.json files from pilot-a-89:
- 37 trials: `reward = 1.0` (pass)
- 52 trials: `reward = 0.0` (fail)
- No intermediate values observed

The verifier runs a task-specific validation script inside the Docker container. It checks whether the agent's work produces the correct output. There is no partial credit — the task either passes or fails.

**Source:** `terminal-bench/submissions/condition-a/pilot-a-89/*/result.json`; `harbor/models/verifier/result.py` (rewards is `dict[str, float | int]`)

### Trial aggregation: Mean (= accuracy = pass rate)

Harbor's default metric is **Mean** (`harbor/metrics/mean.py`):

```python
class Mean(BaseMetric):
    def compute(self, rewards):
        values = []
        for reward in rewards:
            if reward is None:
                values.append(0)    # exceptions/missing → 0
            else:
                values.extend(reward.values())
        return {"mean": sum(values) / len(values)}
```

Since rewards are binary (0.0 or 1.0), the mean equals the **pass rate** across all trials. This is what the leaderboard displays as "Accuracy."

**Key detail:** If a trial has no verifier result (e.g., environment build failure), its reward is treated as `None → 0`. This means infrastructure failures count as failures, not as excluded data points.

The default metric is set in `harbor/job.py:301-303`:
```python
if len(metric_list) == 0:
    metrics[name].append(Mean())
```

### Leaderboard ranking metric

The leaderboard at tbench.ai ranks by **accuracy (mean reward) in descending order**, displayed as a percentage with standard error. Example: `81.8% ± 2.0`.

The standard error is computed across all trials for that agent+model combination (89 tasks × K trials = N total trials). For binary outcomes, SE ≈ `sqrt(p*(1-p)/N)` where p = pass rate and N = total trials.

Current top scores (as of 2026-04-08):
| Agent | Model | Accuracy |
|-------|-------|----------|
| ForgeCode | GPT-5.4 | 81.8% ± 2.0 |
| ForgeCode | Claude Opus 4.6 | 81.8% ± 1.7 |
| TongAgents | Gemini 3.1 Pro | 80.2% ± 2.6 |

**Source:** tbench.ai/leaderboard/terminal-bench/2.0 (123 entries as of 2026-04-08)

---

## 3. Turn Limits

### Harbor has no built-in turn limit enforcement

Harbor itself does NOT enforce a turn limit. The `max_turns` parameter is passed through to the agent as a kwarg:

```python
"kwargs": {
    "max_turns": 50,     # ← Our setting (from reproduce.sh)
    "temperature": 0.0
}
```

The agent implementation (our `adapter.py`) is responsible for honoring `max_turns`. The TB 2.0 reference agent uses `max_turns = 1,000,000` (effectively unlimited).

**Source:** `harbor run --help` shows no `--max-turns` flag — it's passed via agent kwargs; `terminal-bench/analysis/condition-f-final-design.md:364`

### Our max_turns = 50 is a severe compliance issue

Our `reproduce.sh` sets `MAX_TURNS=50` (line 30). This is:
- **Agent-side only** — Harbor doesn't enforce it; our adapter enforces it in its agent loop
- **Dramatically lower than reference** — Reference uses 1,000,000
- **Causes ~45% of failures** — Per early-behavior-findings.md, ~45% of failed trials across ALL conditions hit the 50-turn limit

The adapter code (`wg/adapter.py`) implements the turn limit:
- `WorkgraphAgent.__init__` default: 100
- `ConditionDAgent`: 200
- `ConditionEAgent`: 300
- reproduce.sh override: 50 (binding for all Harbor runs)

**Fix:** Set `MAX_TURNS=1000000` in `reproduce.sh` or remove the `--max-turns` kwarg entirely.

**Source:** `terminal-bench/reproduce.sh:30`; `terminal-bench/analysis/tb2-runtime-compliance-audit.md`; `terminal-bench/analysis/early-behavior-findings.md:87-95`

### No benchmark-imposed turn limit

Terminal-Bench 2.0 does not impose a turn limit. The only constraint is the per-task timeout. Agents can use as many turns as they want within the time budget.

---

## 4. Concurrency Rules

### No per-task concurrency restrictions from the benchmark

Terminal-Bench imposes no rules about concurrent agents per task. The benchmark evaluates one agent instance per trial in an isolated Docker container. What happens inside that container (including spawning sub-agents) is unrestricted.

**Multi-agent architectures are explicitly allowed** by the submission rules:
> "Multi-agent and orchestrator architectures are allowed"
> "Retry/convergence loops within a trial are allowed"

**Source:** `terminal-bench/docs/HOWTO-submit-to-leaderboard.md:99-100`

### Concurrency between trials

The `--n-concurrent` flag controls how many trials run in parallel (default: 4). This is a runner-side optimization, not a benchmark constraint. The leaderboard does not measure or report concurrency.

### Wall-clock time vs. agent-time

The benchmark measures **wall-clock time** for timeout purposes. The `agent_execution` timing in `result.json` records:
```json
"agent_execution": {
    "started_at": "2026-04-07T00:00:56.561698Z",
    "finished_at": "2026-04-07T00:01:43.570618Z"
}
```

This is real wall-clock time, not CPU time or token-processing time. If an agent spawns sub-agents within a single trial, they all share the same wall-clock timeout.

**Implication for Condition G (which may spawn sub-agents via wg):** All sub-agent work must complete within the per-task timeout. There is no "pause the clock" mechanism.

---

## 5. Submission Format

### Directory structure

```
submissions/terminal-bench/2.0/<agent-name>__<model>/
  metadata.yaml                    # Hand-written, required
  <job-folder>/                    # Harbor's raw output directory
    config.json                    # Job-level config
    <task-name>__<hash>/          # One directory per trial
      result.json                  # Harbor-generated trial result
      config.json                  # Trial-level config
      agent/                       # Agent logs (agent_loop.ndjson, etc.)
      verifier/                    # Verifier output
      artifacts/                   # Downloaded artifacts
```

### metadata.yaml (required)

```yaml
agent_url: https://github.com/graphwork/wg
agent_display_name: "wg Condition G (context-injected)"
agent_org_display_name: "wg"

models:
  - model_name: minimax-m2.7
    model_provider: openrouter
    model_display_name: "Minimax M2.7"
    model_org_display_name: "Minimax"
```

### Raw trial data is submitted (not aggregated)

You submit the **raw Harbor output** — individual `result.json` files per trial. The leaderboard infrastructure computes accuracy from these files. You do NOT submit pre-aggregated scores.

### Validation bot checks (automated on PR)

1. `timeout_multiplier == 1.0` in every trial's config
2. No agent timeout overrides (`override_timeout_sec`, `max_timeout_sec` must be null)
3. No verifier timeout overrides
4. No resource overrides (`override_cpus`, `override_memory_mb`, `override_storage_mb`)
5. **Minimum 5 trials per task** (hard requirement, `-k 5`)
6. Valid `result.json` in every trial directory
7. Trial directories contain run artifacts

### Submission workflow

1. Fork `https://huggingface.co/datasets/harborframework/terminal-bench-2-leaderboard`
2. Create a branch
3. Add submission under `submissions/terminal-bench/2.0/<agent>__<model>/`
4. Open a Pull Request → bot auto-validates → maintainer reviews → merge
5. Results auto-imported to tbench.ai leaderboard

### Additional rules

- Agents **cannot access** tbench.ai or the terminal-bench GitHub repo (prevents reward hacking)
- Must **scrub API keys and proprietary prompts** before submission (submissions are public)
- Default resources only (1-2 CPUs, 2-4 GB RAM, 10 GB storage per task)

**Source:** `terminal-bench/docs/HOWTO-submit-to-leaderboard.md`; HuggingFace `harborframework/terminal-bench-2-leaderboard` README

---

## Summary: Compliance Gap Analysis

| Rule | Required | Our Current Setting | Compliant? | Severity |
|------|----------|-------------------|------------|----------|
| Per-task timeout | From task.toml (900–12,000s) | Flat 1800s override | **NO** | HIGH |
| timeout_multiplier | 1.0 | 1.0 | YES | — |
| Turn limit | None (reference: 1M) | 50 | **NO** | HIGH |
| Resource overrides | None | None | YES | — |
| Trials per task | ≥ 5 | 1–3 | **NO** | BLOCKING |
| Result format | Harbor result.json | Harbor result.json (A/B/C); stats.json (F) | PARTIAL | HIGH (F only) |
| Run through Harbor | Required | Yes (A/B/C); No (F) | PARTIAL | HIGH (F only) |

### Path to valid submission

1. **Remove `--timeout "$TIMEOUT"`** from `reproduce.sh` (let Harbor use task.toml defaults)
2. **Set `MAX_TURNS=1000000`** in `reproduce.sh` (match reference agent)
3. **Run all conditions through Harbor** (including F/G via their adapter classes)
4. **Run 89 tasks × 5 trials** per condition (`-k 5`)
5. **All existing trial data must be re-run** — the turn cap and timeout violations invalidate prior results

---

## Sources

| Source | What it provides |
|--------|-----------------|
| `terminal-bench/docs/HOWTO-submit-to-leaderboard.md` | Submission format, rules, workflow |
| `terminal-bench/docs/research-howto-submission-review.md` | Gap analysis, readiness assessment |
| `terminal-bench/docs/research-tb-agent-turn-budget.md` | Turn limit analysis, executor timeouts |
| `terminal-bench/analysis/tb2-runtime-compliance-audit.md` | Full compliance audit (6 constraints) |
| `terminal-bench/analysis/early-behavior-findings.md` | Turn limit impact (~45% of failures) |
| `terminal-bench/docs/scale-experiment-design.md` | Full-scale experiment architecture |
| `terminal-bench/reproduce.sh` | Current runner config (MAX_TURNS=50, TIMEOUT=1800) |
| `~/.cache/harbor/tasks/*/task.toml` | Per-task timeout and resource definitions |
| `harbor/trial/trial.py` | Timeout handling, verifier-after-timeout behavior |
| `harbor/metrics/mean.py` | Aggregation: mean reward = accuracy |
| `harbor/models/verifier/result.py` | Reward structure: `dict[str, float]` |
| `terminal-bench/submissions/condition-a/pilot-a-89/*/result.json` | Binary reward values (0.0 or 1.0) |
| tbench.ai/leaderboard/terminal-bench/2.0 | Live leaderboard (123 entries, ranked by accuracy) |
| HuggingFace `harborframework/terminal-bench-2-leaderboard` | Validation bot rules |
