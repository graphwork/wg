# OpenRouter Leaderboard API & Metrics Research

**Date:** 2026-04-01
**Task:** or-leaderboard-research

## Executive Summary

OpenRouter exposes model quality data through two channels: (1) its own `/api/v1/models` endpoint (pricing/architecture only, no benchmarks), and (2) Artificial Analysis benchmark scores embedded in the rankings page SSR payload. For structured API access to benchmark scores, the Artificial Analysis API (`/api/v2/data/llms/models`) is the authoritative source — free, rate-limited to 1,000 req/day.

**Recommendation for WG:** Use the OpenRouter `/api/v1/models` API for pricing, context window, and model availability. Use the Artificial Analysis API for quality scoring (coding_index and intelligence_index are the most relevant metrics). Parse OpenRouter's SSR payload for throughput/latency if needed, but this is fragile.

---

## 1. OpenRouter API Endpoints

### 1.1 `/api/v1/models` (Documented, Public)

**URL:** `GET https://openrouter.ai/api/v1/models`
**Auth:** None required
**Rate limit:** Unknown (generous for unauthenticated)

Returns ~350 models. Per-model schema:

| Field | Type | Example |
|-------|------|---------|
| `id` | string | `"anthropic/claude-opus-4.6"` |
| `canonical_slug` | string | `"anthropic/claude-opus-4.6-20260205"` |
| `name` | string | `"Anthropic: Claude Opus 4.6"` |
| `context_length` | int | `200000` |
| `architecture.modality` | string | `"text+image->text"` |
| `architecture.input_modalities` | string[] | `["image", "text"]` |
| `architecture.output_modalities` | string[] | `["text"]` |
| `architecture.tokenizer` | string | `"Claude"` |
| `pricing.prompt` | string | `"0.000015"` (USD per token) |
| `pricing.completion` | string | `"0.000075"` |
| `pricing.input_cache_read` | string | per-token cache read cost |
| `top_provider.context_length` | int | best provider's context |
| `top_provider.max_completion_tokens` | int | max output tokens |
| `supported_parameters` | string[] | `["temperature", "tools", ...]` |
| `knowledge_cutoff` | string? | `"2025-04"` |
| `hugging_face_id` | string | HF model ID if applicable |

**Notable absence:** No benchmark scores, quality ratings, ELO, throughput, or latency data.

### 1.2 `/api/v1/models/count` (Documented, Public)

Returns `{"count": 350}` (approximate).

### 1.3 `/api/v1/models/user` (Documented, Requires Auth)

Same schema as `/api/v1/models` but filtered by user preferences and provider settings.

### 1.4 No Public Rankings/Benchmark API

The following URL patterns all return HTML (Next.js SSR pages), **not JSON**:
- `/api/v1/rankings`
- `/api/v1/leaderboard`
- `/api/v1/benchmarks`
- `/api/v1/arena`
- `/api/v1/stats`
- `/api/frontend/rankings`
- `/api/rankings`

**Conclusion:** OpenRouter has no documented or undocumented JSON API for rankings or benchmark data.

---

## 2. OpenRouter Rankings Page Data (SSR-Embedded)

The page at `https://openrouter.ai/rankings` embeds structured data in its React Server Components (RSC) payload. This data is **not stable** — it's embedded in escaped JSON within `self.__next_f.push()` script blocks and changes with each page deployment.

### 2.1 Benchmark Scores (from Artificial Analysis)

Three benchmark categories, each with top-20 models. The `aa_name` field confirms these originate from Artificial Analysis.

Per-model fields in benchmark arrays:
- `uid` — model identifier (e.g., `"openai/gpt-5.4"`)
- `permaslug` — permanent slug
- `openrouter_slug` — OpenRouter model ID
- `heuristic_openrouter_slug` — fallback slug (often null)
- `aa_name` — Artificial Analysis display name
- `score` — numeric score (0-100 scale)

#### Intelligence (top 10 as of 2026-04-01)

| Model | Score |
|-------|-------|
| openai/gpt-5.4 | 57.2 |
| google/gemini-3.1-pro-preview | 57.2 |
| openai/gpt-5.3-codex | 54.0 |
| anthropic/claude-opus-4.6 | 53.0 |
| anthropic/claude-sonnet-4.6 | 51.7 |
| openai/gpt-5.2 | 51.3 |
| z-ai/glm-5 | 49.8 |
| anthropic/claude-opus-4.5 | 49.7 |
| minimax/minimax-m2.7 | 49.6 |
| xiaomi/mimo-v2-pro | 49.2 |

#### Coding (top 10)

| Model | Score |
|-------|-------|
| openai/gpt-5.4 | 57.3 |
| google/gemini-3.1-pro-preview | 55.5 |
| openai/gpt-5.3-codex | 53.1 |
| openai/gpt-5.4-mini | 51.5 |
| anthropic/claude-sonnet-4.6 | 50.9 |
| openai/gpt-5.2 | 48.7 |
| anthropic/claude-opus-4.6 | 48.1 |
| anthropic/claude-opus-4.5 | 47.8 |
| google/gemini-2.5-pro-exp-03-25 | 46.7 |
| google/gemini-3-pro-preview | 46.5 |

#### Agentic (top 10)

| Model | Score |
|-------|-------|
| openai/gpt-5.4 | 69.4 |
| anthropic/claude-opus-4.6 | 67.6 |
| z-ai/glm-5-turbo | 66.1 |
| z-ai/glm-5 | 63.1 |
| anthropic/claude-sonnet-4.6 | 63.0 |
| xiaomi/mimo-v2-pro | 62.8 |
| openai/gpt-5.3-codex | 62.2 |
| minimax/minimax-m2.7 | 61.5 |
| openai/gpt-5.2 | 60.2 |
| anthropic/claude-opus-4.5 | 59.6 |

### 2.2 Performance Data (Throughput & Latency)

139 models with live performance metrics embedded in SSR.

Per-model schema:

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Canonical model ID |
| `slug` | string | URL slug |
| `name` | string | Display name |
| `author` | string | Provider/author |
| `request_count` | int | Total requests (observation period) |
| `p50_latency` | int/float | Median TTFT in milliseconds |
| `p50_throughput` | int/float | Median tokens/second |
| `best_latency_provider` | string | Provider with lowest latency |
| `best_latency_price` | int/float | Price tier of best latency provider |
| `best_throughput_provider` | string | Provider with highest throughput |
| `best_throughput_price` | int/float | Price tier of best throughput provider |
| `provider_count` | int | Number of providers serving this model |

#### Top 10 by throughput

| Model | P50 Throughput (tok/s) | P50 Latency (ms) | Providers |
|-------|----------------------|-------------------|-----------|
| gpt-oss-safeguard-20b | 734 | 179 | 1 |
| gpt-oss-20b | 648 | 92 | 14 |
| Qwen3 32B | 460 | 210 | 9 |
| gpt-oss-120b | 373 | 170 | 19 |
| Llama 3.1 8B Instruct | 265 | 106 | 9 |
| Nemotron 3 Super | 202 | 1550 | 3 |
| Nemotron 3 Nano 30B | 185 | 453 | 2 |
| MiniMax M2.5 | 165 | 575 | 19 |
| Llama 3.3 70B | 145 | 168 | 17 |
| DeepSeek R1 0528 | 144 | 580 | 9 |

### 2.3 Usage/Popularity Data

Weekly token consumption time-series by model and by provider author (Google, Anthropic, OpenAI, etc.), going back ~6 months. Updated weekly.

---

## 3. Artificial Analysis API (Structured Benchmark Source)

### 3.1 Endpoint

**URL:** `GET https://artificialanalysis.ai/api/v2/data/llms/models`
**Auth:** `x-api-key` header (free tier available)
**Rate limit:** 1,000 requests/day (free tier)
**Attribution:** Required ("Powered by Artificial Analysis")

### 3.2 Response Fields

Benchmark scores returned per model:

| Field | Description |
|-------|-------------|
| `artificial_analysis_intelligence_index` | Composite quality score (0-100) |
| `artificial_analysis_coding_index` | Coding-specific composite score |
| `artificial_analysis_math_index` | Math-specific composite score |
| `mmlu_pro` | MMLU-Pro benchmark score |
| `gpqa` | GPQA Diamond (graduate-level scientific Q&A) |
| `hle` | Humanity's Last Exam (frontier academic) |
| `livecodebench` | LiveCodeBench (live coding problems) |
| `scicode` | SciCode (scientific Python generation) |
| `math_500` | MATH-500 benchmark |
| `aime` | AIME (competition math) |

Plus performance metrics: output speed, TTFT latency, pricing.

### 3.3 Intelligence Index v4.0.4 Methodology

10 evaluations in 4 equally-weighted categories (25% each):

**Agents (25%)**
- GDPval-AA (16.7%) — Real-world knowledge work via agentic task completion
- τ²-Bench Telecom (8.3%) — Dual control agent-user simulation

**Coding (25%)**
- Terminal-Bench Hard (16.7%) — Terminal-based agentic task execution
- SciCode (8.3%) — Python code generation for scientific problems

**General (25%)**
- AA-LCR (6.25%) — Long context reasoning
- AA-Omniscience (12.5%) — Knowledge and hallucination assessment
- IFBench (6.25%) — Instruction following precision

**Scientific Reasoning (25%)**
- HLE (12.5%) — Humanity's Last Exam
- GPQA Diamond (6.25%) — Graduate-level scientific Q&A
- CritPt (6.25%) — Physics reasoning

**Confidence:** ±1% at 95% CI. Temperature 0 for non-reasoning, 0.6 for reasoning models. Pass@1 scoring.

### 3.4 Update Frequency

Performance data (speed, latency): Live, based on past 72 hours, measured 8×/day for single requests, 2×/day for parallel.
Benchmark scores: Updated as new models are added; methodology versions change periodically (currently v4.0.4).

---

## 4. Metric Relevance for WG Agent Tasks

### 4.1 Relevance Ranking

| Rank | Metric | Relevance | Why |
|------|--------|-----------|-----|
| **1** | `coding_index` | **Critical** | Direct measure of coding ability — the primary WG agent task |
| **2** | `intelligence_index` | **Critical** | Composite including agents, coding, reasoning, instruction following |
| **3** | Agentic score (SSR only) | **High** | Measures tool use and multi-step task completion — core agent behavior |
| **4** | `livecodebench` | **High** | Live coding problems; closest to real development tasks |
| **5** | IFBench (via intelligence) | **High** | Instruction following — agents must follow task descriptions precisely |
| **6** | `context_length` | **High** | Agents need large context for codebase understanding |
| **7** | `p50_throughput` | **Medium** | Faster models = faster task completion, but quality matters more |
| **8** | `gpqa` | **Medium** | Graduate-level reasoning — relevant for complex design/research tasks |
| **9** | `pricing` | **Medium** | Cost matters for sustained agent operation at scale |
| **10** | `math_index` | **Low** | Rarely relevant to WG's software engineering tasks |
| **11** | `p50_latency` | **Low** | TTFT matters less for long-running agent tasks |
| **12** | `request_count` (popularity) | **Low** | Popularity ≠ quality; can indicate reliability/availability |
| **13** | `hle` | **Low** | Frontier academic knowledge — not typical agent work |

### 4.2 Recommended Composite for WG

For a "WG agent quality score," weight:
- **50%** coding_index — primary work is code
- **30%** intelligence_index — overall reasoning/instruction capability
- **20%** agentic score — tool use and multi-step planning

Alternatively, use the coding_index alone as the primary signal and intelligence_index as tiebreaker.

---

## 5. Data Access Strategy

### 5.1 Recommended Approach

1. **Model catalog + pricing:** OpenRouter `/api/v1/models` (free, unauthenticated, stable JSON API)
2. **Quality benchmarks:** Artificial Analysis `/api/v2/data/llms/models` (free, requires API key, structured JSON)
3. **Join on model ID:** Match `openrouter_slug` from AA to `id` from OpenRouter

### 5.2 Fallback Approach (if AA API is unavailable)

Parse the OpenRouter rankings page SSR payload:
- Benchmark scores in `self.__next_f.push()` blocks with `\\\"score\\\":` patterns
- Performance data in arrays with `p50_latency`/`p50_throughput` fields
- **Fragile:** Breaks on any frontend deployment; no stability guarantees

### 5.3 What Requires Scraping (No API)

- Agentic benchmark scores (only in OpenRouter SSR, not in AA API fields listed)
- Per-model usage/popularity time series
- Provider-level performance comparisons
- Programming language usage breakdown

---

## 6. Update Frequency Summary

| Data Source | Frequency | Notes |
|-------------|-----------|-------|
| OpenRouter `/api/v1/models` | Near-realtime | Models added/removed as providers onboard |
| AA benchmark scores | Per-model evaluation | Updated when new models benchmarked or methodology changes |
| AA performance metrics | Every 3-12 hours | Live, rolling 72-hour window |
| OpenRouter usage rankings | Weekly | Weekly token consumption snapshots |
| OpenRouter SSR performance data | Unknown | Likely daily or weekly refresh |

---

## 7. Open Questions

1. **AA API key acquisition:** Need to register at artificialanalysis.ai for free API access
2. **Agentic score API:** The AA API docs mention `intelligence_index`, `coding_index`, `math_index` — is there also an `agentic_index`? Need to verify with actual API response
3. **Model ID mapping:** Some AA UIDs differ from OpenRouter IDs (e.g., `"openai/gpt-5.4"` vs `"openai/gpt-5.4-20260301"`) — need a fuzzy matching strategy
4. **Score staleness:** How quickly do scores appear for newly released models?
