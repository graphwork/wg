# Model Registry Schema & Update Trace Design

**Date:** 2026-04-01
**Task:** or-registry-design
**Depends on:** or-leaderboard-research

## 1. Overview

This design extends workgraph's existing model registry (`ModelRegistryEntry` in `config.rs`, `ModelEntry` in `models.rs`) with benchmark scores, popularity metrics, and a composite fitness score. It also specifies a workgraph-native update cycle that keeps the registry current without external cron.

### Design Principles

- **Additive, not replacement.** The existing `config.toml` registry and `models.yaml` continue to work. Benchmark data lives in a new sidecar file that enriches existing entries.
- **Two-source strategy.** Pricing/architecture from OpenRouter `/api/v1/models` (free, unauthenticated). Quality benchmarks from Artificial Analysis `/api/v2/data/llms/models` (free, 1k req/day, API key required).
- **Offline-safe.** The registry always has a working static fallback. Benchmark data is optional enrichment, never a hard dependency.

---

## 2. Model Registry Schema

### 2.1 New file: `.workgraph/model_benchmarks.json`

This file stores benchmark and popularity data fetched from external APIs. It is separate from `config.toml` (user-configured model routing) and `models.yaml` (static catalog) to keep concerns clean. It is machine-managed and should not be hand-edited.

```jsonc
{
  "version": 1,
  "fetched_at": "2026-04-01T17:00:00Z",
  "source": {
    "openrouter_api": "https://openrouter.ai/api/v1/models",
    "artificial_analysis_api": "https://artificialanalysis.ai/api/v2/data/llms/models"
  },
  "models": {
    "anthropic/claude-opus-4-6": {
      // === Identity (from OpenRouter /api/v1/models) ===
      "id": "anthropic/claude-opus-4-6",
      "name": "Anthropic: Claude Opus 4.6",

      // === Pricing (from OpenRouter, per million tokens, USD) ===
      "pricing": {
        "input_per_mtok": 15.0,
        "output_per_mtok": 75.0,
        "cache_read_per_mtok": 1.5,    // null if unavailable
        "cache_write_per_mtok": 18.75   // null if unavailable
      },

      // === Architecture (from OpenRouter) ===
      "context_window": 200000,
      "max_output_tokens": 32000,
      "modality": "text+image->text",
      "supports_tools": true,
      "supports_streaming": true,

      // === Benchmarks (from Artificial Analysis API) ===
      "benchmarks": {
        "intelligence_index": 53.0,   // AA composite (0-100)
        "coding_index": 48.1,         // AA coding composite
        "math_index": null,           // AA math composite (null if unavailable)
        "agentic": 67.6,             // OpenRouter SSR only (null if AA unavailable)
        "livecodebench": null,        // Individual benchmark (if exposed by AA)
        "gpqa": null,                 // Graduate-level Q&A
        "ifbench": null               // Instruction following
      },

      // === Performance (from OpenRouter SSR or AA) ===
      "performance": {
        "p50_throughput_tps": 45.0,   // tokens/second, null if unknown
        "p50_latency_ms": 850,        // TTFT in ms, null if unknown
        "provider_count": 3           // number of OpenRouter providers
      },

      // === Popularity (from OpenRouter SSR) ===
      "popularity": {
        "request_count": 1500000,     // total requests in observation period
        "weekly_rank": 4              // rank by weekly token consumption, null if unknown
      },

      // === Computed ===
      "fitness": {
        "score": 72.3,               // composite fitness score (0-100)
        "components": {
          "quality": 52.7,            // weighted benchmark composite
          "value": 18.2,              // quality-adjusted cost efficiency
          "reliability": 1.4          // availability/provider signal
        }
      },

      // === Classification ===
      "tier": "frontier",             // frontier | mid | budget (maps to config.rs Tier)

      // === Staleness tracking ===
      "benchmark_updated_at": "2026-04-01T12:00:00Z",
      "pricing_updated_at": "2026-04-01T17:00:00Z"
    }
  }
}
```

### 2.2 Schema field rationale

| Field group | Source | Why |
|------------|--------|-----|
| `pricing` | OpenRouter API | Stable, structured, free. Drives cost optimization. |
| `benchmarks.coding_index` | AA API | #1 relevance metric per research — primary workgraph agent work is code. |
| `benchmarks.intelligence_index` | AA API | #2 relevance — composite of agents, coding, reasoning, instruction following. |
| `benchmarks.agentic` | OpenRouter SSR | #3 relevance — tool use and multi-step planning. Fragile source, so nullable. |
| `performance` | OpenRouter SSR / AA | Medium relevance — faster agents finish tasks sooner, but quality > speed. |
| `popularity` | OpenRouter SSR | Low direct relevance, but signals reliability and provider investment. |
| `fitness` | Computed | The actionable signal. Single number for model selection decisions. |
| `tier` | Computed | Maps to existing `config.rs` `Tier` enum (`fast`/`standard`/`premium`). |

### 2.3 Tier classification rules

The tier field in the benchmark file bridges to the existing `Tier` enum in `config.rs`:

| Benchmark tier | Config tier | Criteria |
|---------------|-------------|----------|
| `frontier` | `premium` | `fitness.score >= 65` OR `coding_index >= 48` AND `intelligence_index >= 50` |
| `mid` | `standard` | `fitness.score >= 40` OR `coding_index >= 35` |
| `budget` | `fast` | Everything else |

These thresholds are calibrated against the current leaderboard (2026-04-01) where top models score 50-57 on coding_index. They should be reviewed when methodology versions change.

---

## 3. Composite Fitness Score

### 3.1 Formula

```
fitness = quality * 0.70 + value * 0.20 + reliability * 0.10
```

Where:

```
quality = coding_index * 0.50
        + intelligence_index * 0.30
        + agentic * 0.20

value = quality / cost_factor
        (normalized to 0-100 scale)

cost_factor = (input_per_mtok * 0.3 + output_per_mtok * 0.7)
              / median_cost_across_all_models

reliability = min(provider_count / 5, 1.0) * 50
            + min(request_count / 1_000_000, 1.0) * 50
```

### 3.2 Weight rationale

| Component | Weight | Why |
|-----------|--------|-----|
| **Quality** | 70% | Workgraph agents must produce correct code. A cheap model that fails tasks wastes more money than an expensive model that succeeds. Agent sessions run thousands of tokens, so the per-token cost is dwarfed by the cost of wasted agent time on failures + retries. |
| **Value** | 20% | Cost matters at scale — running 10 parallel agents on frontier models adds up. But only after quality is assured. The `value` component rewards models that punch above their price point (e.g., DeepSeek, Qwen). |
| **Reliability** | 10% | Multiple providers = less downtime. High request count = battle-tested. But this is a hygiene factor, not a differentiator. |

### 3.3 Handling missing benchmarks

- If `coding_index` is null: use `intelligence_index * 0.9` as proxy (coding is a subset of intelligence).
- If `intelligence_index` is null: use `coding_index * 1.1` (capped at 100).
- If `agentic` is null: redistribute weight equally to coding and intelligence (55%/45%).
- If ALL benchmark scores are null: `fitness.score = null` — model is unscored and excluded from automated selection.
- Missing performance/popularity fields: the component scores 0 for that sub-factor, other components are renormalized.

### 3.4 Example calculations (2026-04-01 data)

| Model | coding | intelligence | agentic | quality | cost_factor | value | reliability | **fitness** |
|-------|--------|-------------|---------|---------|-------------|-------|-------------|-------------|
| openai/gpt-5.4 | 57.3 | 57.2 | 69.4 | 59.5 | 3.2x | 18.6 | 8.0 | **46.0** |
| anthropic/claude-opus-4-6 | 48.1 | 53.0 | 67.6 | 53.0 | 2.8x | 18.9 | 7.5 | **39.9** |
| anthropic/claude-sonnet-4-6 | 50.9 | 51.7 | 63.0 | 53.5 | 1.4x | 38.2 | 8.0 | **46.0** |
| google/gemini-3.1-pro | 55.5 | 57.2 | — | 56.3* | 1.0x | 56.3 | 7.0 | **51.1** |
| deepseek/deepseek-chat | — | — | — | null | 0.1x | null | 6.0 | **null** |

*Note: Gemini's missing agentic score is redistributed (55%/45% coding/intelligence). Actual fitness would be calculated with real cost/popularity data from the API. These numbers are illustrative.*

---

## 4. Update Trace Design (Workgraph Cycle)

The update trace is a workgraph cycle — a set of tasks connected by back-edges — not an external cron job. This keeps all recurring work visible in the graph.

### 4.1 Cycle structure

```
.registry-fetch-0 → .registry-score-0 → .registry-diff-0 → .registry-fetch-0
                                                    ↓
                                              .registry-smoke-0 (conditional)
```

Created via:

```bash
# Create the cycle with a 24-hour delay between iterations
wg add ".registry-fetch-0" \
  --verify "model_benchmarks.json exists and is valid JSON with >0 models" \
  --max-iterations 0 \
  --cycle-delay 86400 \
  --model claude:haiku \
  -d "## Description
Fetch fresh model data from OpenRouter and Artificial Analysis APIs.

## Steps
1. Call OpenRouter /api/v1/models → extract pricing, context_window, modality, tool support
2. Call Artificial Analysis /api/v2/data/llms/models → extract benchmark scores
3. Join on model ID (fuzzy match: strip date suffixes, normalize slashes)
4. Write raw fetched data to .workgraph/model_benchmarks_raw.json

## Validation
- [ ] model_benchmarks_raw.json contains pricing for >= 100 models
- [ ] model_benchmarks_raw.json contains benchmark scores for >= 20 models
- [ ] All API calls completed without error"

wg add ".registry-score-0" --after ".registry-fetch-0" \
  --verify "model_benchmarks.json has fitness.score for all models with benchmarks" \
  --model claude:haiku \
  -d "## Description
Compute composite fitness scores and tier classifications.

## Steps
1. Read .workgraph/model_benchmarks_raw.json
2. Compute fitness score per model (formula in design doc)
3. Classify tiers based on thresholds
4. Write .workgraph/model_benchmarks.json (the canonical registry)

## Validation
- [ ] Every model with coding_index OR intelligence_index has a fitness.score
- [ ] Tier distribution is reasonable (not all frontier or all budget)
- [ ] model_benchmarks.json validates against schema v1"

wg add ".registry-diff-0" --after ".registry-score-0" \
  --verify "diff report written to .workgraph/model_benchmarks_diff.md" \
  --model claude:haiku \
  --back-edge ".registry-fetch-0" \
  -d "## Description
Compare current registry with previous version and flag changes.

## Steps
1. Load previous model_benchmarks.json (from git or backup)
2. Diff against new version
3. Flag: new models entering top-N, models dropping out of top-N, tier changes, >5% score swings
4. Write .workgraph/model_benchmarks_diff.md with summary
5. If a model was promoted to frontier tier, optionally create a smoke test task

## Validation
- [ ] Diff report lists additions, removals, tier changes, and score deltas
- [ ] If a new model enters top-10 by fitness, it is flagged prominently"
```

### 4.2 Cycle parameters

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| `--max-iterations 0` | Unlimited | Registry should stay up-to-date as long as workgraph is running |
| `--cycle-delay 86400` | 24 hours | AA benchmarks update per-model (not on a fixed schedule). Daily is frequent enough to catch new models within a day while staying well under the 1k/day API limit. |
| `--model claude:haiku` | Budget model | Fetch/score/diff are mechanical tasks — no reasoning needed, minimize cost. |

### 4.3 Smoke test trigger (conditional)

When `.registry-diff-0` detects a model promoted to frontier tier that isn't already in the `config.toml` registry, it creates a one-shot smoke test task:

```bash
wg add ".smoke-test-${MODEL_ID}" \
  --after ".registry-diff-0" \
  --model "${MODEL_ID}" \
  --verify "Agent completes a simple coding task and produces valid output" \
  -d "## Description
Smoke test for newly-promoted model ${MODEL_ID}.
Run a small coding task to verify the model works with workgraph's agent system.

## Validation
- [ ] Model accepts tool_use format
- [ ] Model produces syntactically valid code
- [ ] Model follows task instructions"
```

This is advisory — the test result is logged but doesn't block registry updates.

### 4.4 Manual trigger

The cycle runs automatically when `wg service start` is active. For manual one-shot updates:

```bash
wg spawn .registry-fetch-0   # runs one full fetch→score→diff pass
```

---

## 5. Integration Points

### 5.1 `wg config --model` / `wg model set-default`

**Current behavior:** User specifies a model by ID (e.g., `claude:opus`). The config system resolves it through the registry cascade (`ModelRegistryEntry` → `TierConfig` → fallback).

**Enhanced behavior:** When setting a model, show fitness information as a recommendation:

```
$ wg model set-default claude:sonnet
Set default model to: claude:sonnet
  Fitness: 46.0 (quality: 53.5, value: 38.2, reliability: 8.0)
  Tier: mid — ranked #4 overall by fitness
  Note: claude:opus scores higher on quality (53.0 vs 53.5) but lower on value (18.9 vs 38.2)
```

Implementation: `model_cmd.rs::run_set_default()` loads `model_benchmarks.json` and prints enrichment data after the existing "Set default model to:" message. No change to model resolution logic — this is display-only.

### 5.2 Per-task model assignment (`wg add --model`)

**Current behavior:** `task.model` overrides the dispatch cascade. The coordinator's spawn logic (`execution.rs:1159-1301`) resolves: task model → executor model → role tier → default.

**Enhanced behavior: tier-based smart defaults.** When `--model` is not specified, the coordinator can use the benchmark registry to pick a model appropriate to the task's estimated complexity:

```
Resolution cascade (enhanced):
1. task.model (explicit override — unchanged)
2. task.tier → model_benchmarks.json top-scoring model for that tier
3. agent.role.desired_outcome → tier inference → model_benchmarks.json
4. DispatchRole default_tier() → TierConfig → registry (existing behavior)
```

**Key change:** Step 2 is new. When a task has `tier = "premium"` but no explicit `model`, the system consults `model_benchmarks.json` for the highest-fitness frontier model rather than relying solely on the static `[tiers.premium]` config entry.

Implementation in `config.rs::resolve_model_for_role()`:
- After the existing tier resolution (step 3 in current cascade), add a benchmark-aware fallback.
- Load `model_benchmarks.json` lazily (cache in `Config` struct on first access).
- Filter by matching tier, sort by `fitness.score` descending, return the top entry.
- If `model_benchmarks.json` doesn't exist or is stale (>7 days), fall through to existing behavior.

### 5.3 Agency placement decisions

**Current behavior:** The `.place-*` task uses an LLM to select a model tier for a task. The evolve system's `partition.rs::ModelTier` enum (Haiku/Sonnet/Opus) recommends tiers for analyzer tasks.

**Enhanced behavior:** Placement can reference the benchmark registry to make data-driven decisions:

1. **Placement prompt enrichment.** The `.place-*` task's system prompt can include a summary of available models per tier:
   ```
   Available models by tier:
   - frontier: claude-opus-4-6 (fitness: 72.3), gpt-5.4 (fitness: 70.1)
   - mid: claude-sonnet-4-6 (fitness: 46.0), gemini-3.1-pro (fitness: 51.1)
   - budget: gemini-2.0-flash (fitness: 28.4), deepseek-chat (fitness: null)
   ```
   This helps the LLM make informed tier selections rather than guessing.

2. **Post-placement model resolution.** Once the placer selects a tier, the `resolve_model_for_role()` cascade (enhanced per 5.2) picks the best model within that tier.

3. **Evolution feedback loop.** Evaluation scores from `wg evaluate` can be correlated with model fitness to identify when a model underperforms its benchmark predictions, feeding back into fitness calibration over time.

### 5.4 New CLI commands

```bash
# Show benchmark data for a specific model
wg model info <model-id>
# Output: pricing, benchmarks, fitness, tier, staleness

# Show ranked leaderboard from benchmark registry
wg model leaderboard [--tier <tier>] [--limit N]
# Output: models sorted by fitness score

# Force a registry refresh (triggers the cycle manually)
wg model refresh
# Equivalent to: wg spawn .registry-fetch-0

# Show what model would be selected for a given role
wg model resolve <role>
# Output: resolution cascade trace (existing + benchmark enhancement)
```

### 5.5 `wg models search` enrichment

**Current behavior:** `wg models search` queries OpenRouter `/api/v1/models` and displays pricing/context.

**Enhanced behavior:** If `model_benchmarks.json` exists, join search results with benchmark data to show fitness scores inline:

```
$ wg models search claude
MODEL                                  IN/1M       OUT/1M     CTX  FITNESS  TIER
anthropic/claude-opus-4-6             $15.00      $75.00       200k   72.3  frontier
anthropic/claude-sonnet-4-6            $3.00      $15.00      1M     46.0  mid
anthropic/claude-haiku-4-5             $0.80       $4.00      200k   32.1  budget
```

---

## 6. Data Flow Diagram

```
                    ┌──────────────────────┐
                    │  OpenRouter API       │
                    │  /api/v1/models       │
                    │  (pricing, arch)      │
                    └──────────┬───────────┘
                               │
                               ▼
┌──────────────────┐    ┌─────────────┐    ┌──────────────────────┐
│ Artificial       │───▶│ .registry-  │───▶│ model_benchmarks_    │
│ Analysis API     │    │  fetch-0    │    │ raw.json             │
│ (benchmarks)     │    └─────────────┘    └──────────┬───────────┘
└──────────────────┘                                  │
                                                      ▼
                                              ┌─────────────┐
                                              │ .registry-  │
                                              │  score-0    │
                                              └──────┬──────┘
                                                     │
                                                     ▼
                                           ┌───────────────────┐
                                           │ model_benchmarks. │
                                           │ json (canonical)  │
                                           └────────┬──────────┘
                                                    │
                               ┌────────────────────┼────────────────────┐
                               ▼                    ▼                    ▼
                     ┌─────────────────┐  ┌─────────────────┐  ┌───────────────┐
                     │ resolve_model_  │  │ wg model info/  │  │ .place-* task │
                     │ for_role()      │  │ leaderboard     │  │ prompt context│
                     │ (config.rs)     │  │ (CLI display)   │  │ (agency)      │
                     └─────────────────┘  └─────────────────┘  └───────────────┘
```

---

## 7. Model ID Matching Strategy

OpenRouter and Artificial Analysis use slightly different model IDs. The research noted examples like `openai/gpt-5.4` vs `openai/gpt-5.4-20260301`.

### Matching algorithm

1. **Exact match** on OpenRouter `id` field ↔ AA `openrouter_slug` field.
2. **Prefix match** with date suffix stripped: `openai/gpt-5.4-20260301` → `openai/gpt-5.4`.
3. **Canonical alias table** (manually maintained in `model_benchmarks.json`):
   ```json
   "aliases": {
     "anthropic/claude-3.5-sonnet": "anthropic/claude-sonnet-4-6",
     "openai/chatgpt-4o-latest": "openai/gpt-4o"
   }
   ```

The alias table is small (10-20 entries) and updated as part of the fetch cycle when unmatched models are detected.

---

## 8. File Layout Summary

```
.workgraph/
├── config.toml                    # User-configured model routing (existing)
├── models.yaml                    # Static model catalog (existing, deprecated path)
├── model_benchmarks.json          # Benchmark + fitness registry (new, machine-managed)
├── model_benchmarks_raw.json      # Raw fetched data before scoring (new, intermediate)
├── model_benchmarks_diff.md       # Latest change report (new, human-readable)
└── model_cache.json               # OpenRouter API response cache (existing)
```

---

## 9. Migration Path

1. **Phase 1 (this design):** `model_benchmarks.json` is an optional sidecar. All existing model selection continues to work. The benchmark file is only consulted when it exists and is fresh.
2. **Phase 2 (or-registry-impl):** Implement the fetch/score/diff cycle tasks and the `wg model info/leaderboard` commands. Wire benchmark data into `resolve_model_for_role()` as a soft fallback.
3. **Phase 3 (future):** Consolidate `models.yaml` and `model_benchmarks.json` into a single source of truth. Deprecate the static `models.yaml` in favor of the auto-updated benchmark registry. The `config.toml` model_registry entries continue to serve as user overrides.

---

## 10. Open Questions for Implementation

1. **AA API key management.** Where should the Artificial Analysis API key be stored? Options: `config.toml`, environment variable (`AA_API_KEY`), or `.workgraph/secrets/`. Recommendation: environment variable, consistent with `OPENAI_API_KEY` pattern.
2. **Cycle bootstrap.** The `.registry-fetch-0` cycle needs to be created on `wg init` or on first `wg service start`. Recommendation: create lazily on first `wg service start` if it doesn't exist.
3. **Stale threshold.** How old can `model_benchmarks.json` be before it's ignored by `resolve_model_for_role()`? Recommendation: 7 days (configurable in `config.toml`).
4. **Agentic score source.** The AA API may expose an `agentic_index` field not documented in the research. The implementation should check the actual API response and use it if available, falling back to OpenRouter SSR scraping if not.
