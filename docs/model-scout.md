# Model-scout: re-runnable OpenRouter strong/weak tier picker

**Task:** model-scout-re
**Status:** Implemented (verb `wg model-scout`; engine behind `wg profile pi --scout`)
**Depends on:** [design-two-tier-pi-profile.md](design-two-tier-pi-profile.md) (the tier interface it writes to)
**Code:** `src/model_scout.rs` · **CLI:** `src/cli.rs` (`Commands::ModelScout`) · **dispatch:** `src/main.rs`

---

## TL;DR

OpenRouter model identity is a moving target. The two Pi tiers (`strong` / `weak`)
are the *stable* interface; the model **behind** each tier moves. The model-scout
is the forward-looking mechanism that repoints those two tiers **without
hardcoding** any model id — re-run it whenever the market moves.

```
wg model-scout                       # dry-run (default): research + propose, write nothing
wg model-scout --apply               # write the proposed tiers
wg model-scout --max-cost 5          # cap both tiers to $5/Mtok blended (cost-bounded profile)
wg model-scout --json                # machine-readable proposal (for scheduled re-runs)
```

It **always says what it is doing**, one line per tier:

```
strong: <old> -> <new>  because …
weak:   <old> -> <new>  because …
```

It **bootstraps from whatever tiers are currently set** (the incumbents are the
baseline to beat), and every change prints a one-command revert. The dry-run
report and the `--apply` write are generated from the *same* proposal, so what it
shows is exactly what it writes (legibility ⇄ reversibility).

This is the research/selection engine behind the design's
`wg profile pi --scout` entry point: that flag delegates to
[`worksgood::model_scout::scout`](../src/model_scout.rs).

---

## The two tiers and what each optimizes

| Tier | Drives (per [design §4](design-two-tier-pi-profile.md)) | Objective |
|------|--------------------------------------------------------|-----------|
| **strong** | chat, worker (TaskAgent), evolver, creator, verification — and the `standard`/`premium` tier roles | **best coding/work model right now** (quality matters) |
| **weak** | `.flip` / `.assign` / post-flip evaluation / off-the-rails detection — and the `fast` tier roles | **cheapest model reliable enough** for recoverable judgment one-shots |

The asymmetry is the whole point. `strong` is **capability-first**: it produces
the work product, so quality dominates. `weak` is **reliability-at-low-cost, not
raw capability**: agency calls are one-shots where a wrong verdict is cheap to
correct, so the scout minimizes cost *subject to a reliability floor* rather than
chasing capability.

---

## Selection criteria (documented, **not** hardcoded to model ids)

All preferences are *rules over OpenRouter metadata* (pricing, context window,
advertised capabilities) — there is intentionally **no allow-list of model ids**.
The criteria live in one place, the `criteria` module of `src/model_scout.rs`,
so they can be tuned (or replaced with a measured benchmark) without touching the
control flow.

### Shared candidate filter

A catalog entry is dropped before scoring if any of:

- it is a floating `~vendor/model` "latest" alias (unstable identity);
- it has no usable pricing (free `:free` routes, `$0` placeholders — rate-limited
  / unreliable);
- it is a non-text or unstable specialty route (id contains `:free`, `preview`,
  `experimental`, `-alpha`, `-beta`, `-image`, `image-`, `-audio`, `-tts`,
  `-voice`, `-vl-`).

### `strong` — best coding/work model

**Eligibility floor:** advertises `tools` **and** `structured_outputs`,
`context_length ≥ 131072`, passes the shared filter, and (if `--max-cost` is set)
blended cost ≤ ceiling.

**Score (higher = better)** — a transparent capability proxy:

```
0.55 · norm_log(output_price, $1 … $30 /Mtok)   # frontier-price band, saturating
0.25 · norm_log(context,      131k … 1.05M)
0.10 · supports parallel tool calls
0.10 · supports reasoning
```

- The **frontier price band** treats output price as a capability signal but
  *saturates* at the top, so the scout does not simply pick the single most
  expensive "pro" tier.
- The incumbent must be beaten by a **5% margin** before a switch is proposed
  (anti-churn). If the incumbent is already top (or within margin), it stays —
  `(unchanged)`.

> ⚠️ **Honest caveat (extension hook).** `GET /api/v1/models` does **not** expose
> a coding benchmark, so `strong` uses a *spec + frontier-price proxy*. When a
> measured coding index is available (e.g. Artificial Analysis `coding_index`,
> already modeled in `src/model_benchmarks.rs`), it should dominate this proxy.
> Only the `score_strong` function changes — the control flow does not. This is
> surfaced in the scout's output (`note:` line) so the proxy is never mistaken
> for a measurement.

### `weak` — cheapest *reliable* model for agency one-shots

**Reliability floor (must clear ALL):**

- advertises `tools` (function/tool calls land reliably) **and**
  `structured_outputs` (agency verdicts are structured);
- `context_length ≥ 131072` (room for task + judgment output);
- passes the shared filter (no free / preview / specialty routes);
- **not** a `thinking` / `-r1` / reasoning-named model — these inflate
  per-verdict cost and latency on a one-shot, defeating *reliability-at-low-cost*
  (the scout wants *predictable cheap* cost);
- (if `--max-cost` is set) blended cost ≤ ceiling.

**Pick:** the **cheapest** model that clears the floor — `argmin` of blended cost
(`0.3·input + 0.7·output` per Mtok). Tie-break: larger context, then id (stable).
The incumbent must be beaten by being **>10% cheaper** before a switch is
proposed.

This is the explicit encoding of "cheap tier optimizes reliability-at-low-cost,
not raw capability": a hard reliability constraint, then minimize cost.

---

## Bootstrap: the incumbents are the baseline to beat

The scout reads the current tiers from config (per the design read-path):

- `strong ← agent.model` (fallback `[models.default].model`)
- `weak ← tiers.fast` (fallback `[models.evaluator].model`)

Each incumbent is looked up in the live catalog and scored on the *same* rubric.
A tier only changes when a candidate beats the incumbent by the tier's switch
margin. If an incumbent is a non-OpenRouter spec (`claude:` / `codex:` / `nex:`),
it cannot be scored against the catalog, so the scout adopts the best eligible
OpenRouter model and says why.

---

## Output, apply, and reversibility

- **Dry-run (default):** prints the proposal block + the exact copy-pasteable
  apply command, in both `wg model-scout --apply` form and the canonical
  `wg profile pi --strong … --weak …` form (partial-flag form when only one tier
  changes — the common case).
- **`--apply`:** writes only the changed tier(s). `strong` writes the work /
  default / task-agent / `tiers.standard` / `tiers.premium` keys (via
  `Config::pin_default_route_model`); `weak` writes `tiers.fast` plus the four
  agency-role overrides (`evaluator`, `assigner`, `flip_inference`,
  `flip_comparison`) — the exact key-set from [design §4.1](design-two-tier-pi-profile.md#41-which-config-keys-each-tier-writes).
  It prints the one-command revert (the same command with the old values).
- The proposal is computed once and reused for both the report and the write, so
  **the report and the apply always match**.

> The self-contained writer makes `--apply` real and testable today. Once the
> canonical `wg profile pi` setter lands (task `interactive-pi-model`), the
> two-tier write becomes a thin call into that setter, which additionally handles
> named-profile files and daemon hot-reload (per [design §6](design-two-tier-pi-profile.md#6-persistence--reload)).

---

## Re-runnable on demand

### As a verb

`wg model-scout` is launchable any time the market moves. It is read-only by
default, so it is always safe to run for a recommendation.

### As a scheduled / launchable task (tag `eval-scheduled`)

Fire it as a WG task whenever you want a forward-looking check, or wire it on a
cadence:

```bash
# one-shot launchable task
wg add 'Scout: refresh Pi strong/weak tiers' \
  -d '## Description
Run `wg model-scout --json` and review the proposal. If it recommends a change
and the reasoning holds, apply it with `wg model-scout --apply` (or the printed
`wg profile pi` command) and note the one-command revert.

## Validation
- [ ] `wg model-scout --json` produced a strong+weak proposal with reasoning
- [ ] Any applied change recorded the revert command'
```

For a recurring cadence, schedule the same `wg model-scout --json` invocation
(cron / `wg`-scheduled task) and have a reviewer approve before `--apply`. The
JSON output (`{fetched, baseline_*, strong:{old,new,changed,reason}, weak:{…}}`)
is stable for programmatic diffing between runs.

---

## Worked example (live, 2026-06-23)

Baseline `strong=openrouter:z-ai/glm-5.2`, `weak=openrouter:deepseek/deepseek-chat`:

```
$ wg model-scout
DRY RUN — no files written.
Scouting OpenRouter for the two Pi tiers (340 models fetched).
  baseline: strong=openrouter:z-ai/glm-5.2, weak=openrouter:deepseek/deepseek-chat

Proposed Pi profile tiers
  strong: openrouter:z-ai/glm-5.2 -> openrouter:openai/gpt-5.5
          because higher capability proxy (score 0.90 vs incumbent 0.53);
          tools+structured, 1.0M ctx, $30.00/Mtok out (frontier band).
  weak: openrouter:deepseek/deepseek-chat -> openrouter:inclusionai/ling-2.6-flash
          because clears the agency reliability floor (tools+structured, 256k ctx,
          stable non-free, non-thinking) at $0.024/Mtok blended vs $0.620 — 96%
          cheaper for flip/assign/eval/off-the-rails one-shots.

Apply with:
  wg model-scout --apply
  # canonical equivalent (per design-two-tier): wg profile pi --strong openrouter:openai/gpt-5.5 --weak openrouter:inclusionai/ling-2.6-flash
```

With a cost ceiling, the strong tier stays put because the frontier model is
filtered out — same criteria, different budget:

```
$ wg model-scout --max-cost 5
  strong: openrouter:z-ai/glm-5.2 -> openrouter:z-ai/glm-5.2  (unchanged)
          because still at the capability frontier (score 0.53; …); no eligible
          candidate beats it by the 5% switch margin.
  weak:   openrouter:deepseek/deepseek-chat -> openrouter:inclusionai/ling-2.6-flash
          because … 96% cheaper …
```
