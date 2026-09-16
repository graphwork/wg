# Design: Retire the WG model registry — source model truth from Pi

**Task:** design-retire-the
**Date:** 2026-09-16
**Status:** Proposed (design only — names code to change, does NOT implement)
**Implementation task:** implement-retire-the (Milestone 1 below is its authorized scope)
**Depends on:** [design-handler-first-model-spec.md](design-handler-first-model-spec.md), [design-two-tier-pi-profile.md](design-two-tier-pi-profile.md), [config-ux-design.md](config-ux-design.md)

---

## TL;DR

WG today maintains **four overlapping "registry" systems** that all try to answer
the same question — *what models exist, what do they cost, what can they do?* —
none of which is actually authoritative:

1. `[[model_registry]]` config entries (`Config.model_registry: Vec<ModelRegistryEntry>`)
2. `.wg/models.yaml` (`src/models.rs::ModelRegistry`, hardcoded defaults)
3. `.wg/model_benchmarks.json` (`model_benchmarks.rs`), populated by a **daemon
   background loop that fetches OpenRouter with a provider API key**
4. Ad-hoc OpenRouter discovery (`wg models search/remote`, `wg model-scout`)

Meanwhile **Pi already owns the real catalog**: `~/.pi/agent/models-store.json`
is maintained, refreshed, and versioned by Pi itself — per-provider model lists
with pricing, context windows, and capability metadata, kept fresh by Pi's own
update machinery. WG duplicates a stale, key-gated, half-maintained copy of data
Pi already has.

**Decision:** Pi's `models-store.json` becomes the **single source of truth** for
model catalog data (provider/model identity, $/Mtok rates, context windows).
WG deletes its own catalog machinery — starting with the daemon registry-refresh
path (the only place WG code demands a provider API key to refresh a catalog) —
and adds one small read-only **Pi-catalog reader**. Cost estimation, `wg config
--models`, and the migrate/lint surface are rewired to it in the same change.
`claude:` / `codex:` native routes keep their existing minimal static handling.

Milestone 1 (this design's implementation task) removes the refresh path + config
registry and lands the reader + rewires. The `.wg/models.yaml` registry and the
OpenRouter discovery verbs (`wg models search/remote`, `wg model-scout`) are
explicit follow-up waves, listed in §7 so nothing is left dangling.

---

## 1. Background: what exists today (the inventory)

Four systems, four files, overlapping and mutually inconsistent:

| # | System | Storage | Freshness | Needs API key? |
|---|--------|---------|-----------|----------------|
| R1 | `Config.model_registry` (`[[model_registry]]` TOML entries) | `worksgood.toml` / `~/.wg/config.toml` | hand-edited, never refreshed | no |
| R2 | `src/models.rs::ModelRegistry` | `.wg/models.yaml` | hardcoded defaults (`with_defaults()`), stale prices (e.g. opus at 5/25 when the real rate is 15/75 in `fallback_model_pricing_mtok`) | no |
| R3 | `model_benchmarks.rs::BenchmarkRegistry` | `.wg/model_benchmarks.json` | **daemon background refresh** (`run_registry_refresh` → OpenRouter `GET /api/v1/models`) | **yes** (`resolve_openai_api_key_from_dir`) |
| R4 | OpenRouter discovery | `.wg/model_cache.json` (1 h) + live fetch | on demand (`wg models search/remote`, `wg model-scout`) | **yes** |

### 1.1 The daemon refresh path (R3) — the worst offender

`src/commands/service/mod.rs` runs `run_registry_refresh` on a timer
(`config.coordinator.registry_refresh_interval`), gated by
`registry_refresh_has_selected_route`, with a failure-threshold/cooldown machine
(`REGISTRY_REFRESH_FAILURE_THRESHOLD = 5`, `REGISTRY_REFRESH_COOLDOWN = 1 h`).
It calls `do_registry_refresh`, which:

- resolves an **OpenRouter API key** from the workgraph dir (`resolve_openai_api_key_from_dir`),
- fetches the full live catalog,
- rebuilds `BenchmarkRegistry`, preserves manual benchmark scores, computes
  fitness scores, diffs, and saves.

Problems:

- **The daemon demands a provider API key for catalog data.** A missing/expired
  key produces recurring background errors and cooldown churn for something the
  daemon does not need to function. This is exactly the surface the
  implementation milestone must eliminate: *no daemon code requests provider
  API keys for catalog refresh.*
- The fitness-ranked output feeds `wg profile pi` auto-configuration and triage
  ranking — but the ranking is only as fresh as the last successful key-bearing
  refresh, and silently stale otherwise.
- The daemon has no other consumer for the data; R3 exists almost entirely to
  feed R4-adjacent ranking.

### 1.2 The config registry (R1) — inert authority, real blast radius

`ModelRegistryEntry` (id/provider/model/tier/endpoint/context_window/cost_*/
prompt_caching/descriptors) is consulted by:

- **Cost estimation:** `graph::infer_model_pricing` → `estimate_agent_cost_usd`
  (`src/graph.rs`). When an agent's provider reports **no cost** (e.g. Pi on a
  gateway route like `lunaroute/glm-5.3-flash` reports `usage.cost.total = 0`),
  WG falls back to estimating from rates — but only if a `[[model_registry]]`
  entry matches, else `fallback_model_pricing_mtok` (three hardcoded specs:
  `codex:gpt-5.5`, `codex:gpt-5.4`, `claude:opus`), else **$0.00**. Every
  provider not in those two lists shows $0.00 forever.
- **Spawn alias resolution:** `spawn/execution.rs::resolve_model_via_registry`
  resolves a bare alias (`my-custom`) to a real model + endpoint via R1.
- **Merge-resolution snapshots** (`merge_resolution/mod.rs`): the merger route's
  strong/premium assertion falls back to the R1 entry's `tier`/`descriptors`.
- **`wg config model add/remove/list`** (`config_cmd.rs`) and the
  `wg config --models` plane; `wg setup` wizard choices and `auto_map_tiers`;
  `wg init` route inheritance; config lint rules (Rule 3/4 in `config.rs`).

After handler-first model specs (design-handler-first-model-spec), R1 is
increasingly vestigial: dispatch authority is the handler-first route, and the
registry only supplies side-band pricing/endpoint/tier metadata that drifts.

### 1.3 The `.wg/models.yaml` registry (R2)

`wg models list/add/set-default/init` + `supports_tool_use` checks in
`tui_nex` / `native_exec` / `nex_runtime`. Entirely separate data from R1, with
its own stale hardcoded prices. Not touched by Milestone 1 (§7 Wave 2).

### 1.4 What Pi already provides

`~/.pi/agent/models-store.json` (Pi agent dir; also mirrored for hermetic
probing in `executor_discovery.rs::pi_registration_revision`, which already
hashes this file) is a per-provider map:

```json
{
  "openrouter": {
    "checkedAt": 1789590888281, "lastModified": 1789562359000,
    "models": [ { "id": "z-ai/glm-5.3-flash", "name": "…", "api": "openai-completions",
                  "provider": "openrouter", "baseUrl": "…", "reasoning": true,
                  "thinkingLevelMap": {…}, "input": ["text"],
                  "cost": { "input": 0.09, "output": 0.3, "cacheRead": 0.015, "cacheWrite": 0 },
                  "contextWindow": 1048576, "maxTokens": 32768, "compat": {…} } ]
  },
  "openai-codex": { "models": [ … "gpt-5.5" cost 5/30 … ] },
  "zai": { … }, "lunaroute": { … }
}
```

Key properties:

- **Cost units match WG's existing convention** (`cost.input` is USD per 1M
  tokens, same unit as `cost_per_input_mtok`) — a direct field mapping, no
  conversion.
- **Pi maintains freshness itself** (`checkedAt`/`lastModified`; Pi refreshes
  the store through its own provider plumbing). WG performs **no network I/O**
  and requests **no API keys** for catalog data, ever.
- Coverage is strictly a superset of WG's hardcoded data: every provider WG can
  reach through `pi:` routes appears (openrouter, openai-codex, zai, lunaroute,
  …), including the ones WG's hardcoded lists have never heard of.
- Caveats encoded in the data itself: sentinel/negative prices
  (`openrouter/auto` at `-1000000`) and genuinely-free entries (`:free`,
  `highspeed` variants at `0`). The reader must distinguish **unknown** from
  **free** (§3 D2).

---

## 2. Goals / non-goals

**Goals**

- G1: No WG daemon code requests a provider API key for catalog refresh. The
  periodic refresh path is deleted, not disabled.
- G2: One catalog source of truth: Pi's `models-store.json`. One small reader
  in WG; every consumer rewired in the same change that removes its old source.
- G3: Cost estimation produces non-zero, catalog-accurate estimates for any
  provider Pi knows (lunaroute, zai, future gateways) instead of $0.00.
- G4: `claude:` / `codex:` native routes keep their existing minimal handling
  (no catalog requirement, existing static fallback pricing) — unchanged
  behavior, no new failure modes.
- G5: Legacy `[[model_registry]]` config migrates cleanly through the standard
  `wg migrate config` path with `wg config lint` clean afterwards.

**Non-goals**

- Retiring the `claude:`/`codex:` CLI handlers or their static fallbacks.
- Any new network fetch in WG (the reader is read-only over a local file).
- Removing `wg models search/remote` or `wg model-scout` in Milestone 1
  (explicit, human-invoked tools; Wave 3, §7).
- Touching the exec/federation trust planes — this is a catalog/plumbing
  retirement only.

---

## 3. Design decisions

### D1 — Pi's models-store.json is the single catalog source of truth

WG stops asserting model catalog facts (existence, pricing, context size) from
its own stores. For anything `pi:`-routed, catalog truth = what the local Pi
installation believes. WG's own copies (R1 pricing fields, R2 defaults, R3
fitness data) become non-authoritative and are retired on the schedule in §7.

### D2 — The reader: `src/pi_catalog.rs`

One small module, no network, no keys:

```rust
pub struct PiModel {
    pub provider: String,      // store section key (e.g. "lunaroute")
    pub id: String,            // e.g. "glm-5.3-flash"
    pub cost_input_per_mtok: Option<f64>,   // None = unknown (missing/negative sentinel)
    pub cost_output_per_mtok: Option<f64>,
    pub cost_cache_read_per_mtok: Option<f64>,
    pub context_window: Option<u64>,
}

pub struct PiCatalog { /* providers: BTreeMap<String, Vec<PiModel>> */ }

/// PI_CODING_AGENT_DIR → $HOME/.pi/agent (same resolution as
/// executor_discovery::pi_registration_revision — one helper, reused).
pub fn agent_dir() -> Option<PathBuf>;
/// Parse models-store.json. Missing file/unparseable ⇒ Ok(empty catalog),
/// never an error at call sites that must not fail accounting.
pub fn load_from_dir(dir: &Path) -> PiCatalog;
pub fn find(&self, provider: &str, model: &str) -> Option<&PiModel>;
/// Route → (provider, model) for handler-first specs:
///   pi:lunaroute/glm-5.3-flash        → ("lunaroute", "glm-5.3-flash")
///   pi:openrouter:z-ai/glm-5.2        → ("openrouter", "z-ai/glm-5.2")
///   pi:zai/glm-5.2                    → ("zai", "glm-5.2")
/// Non-pi specs (claude:/codex:/nex:…) ⇒ None (callers keep their own path).
pub fn split_pi_spec(route: &str) -> Option<(String, String)>;
```

**Missing-entry behavior (the contract):**

- `find` returns `None` for unknown provider/model — callers decide; the
  reader never errors, never guesses, never substitutes.
- Unknown vs free: a **negative or missing** `cost` field ⇒ `None` (unknown);
  an explicit `0` ⇒ `Some(0.0)` (genuinely free — `:free` variants are real).
- File missing / JSON unparseable / schema drift ⇒ empty catalog (treated as
  "no entries"), plus a **single one-line stderr warning** in interactive
  user-facing surfaces (`wg config --models`, catalog listing). The accounting
  path (`estimate_agent_cost_usd`) stays silent and falls through to its
  existing fallback chain — a missing catalog must never break or distort
  accounting, it only removes one input.
- Parsing is defensive: unknown fields ignored (`serde(deny_unknown_fields)`
  is explicitly NOT used — Pi may add fields any release), per-entry failure
  skips the entry, not the catalog.
- Caching: per-process cache keyed on `(path, mtime, len)` — the same pattern
  as `pi_registration_revision` — so hot loops (daemon token accounting) parse
  the file once per file version, not once per task.

### D3 — Cost estimation rewires to catalog rates

`graph::estimate_agent_cost_usd` keeps its signature and its contract
(provider-reported cost is persisted exactly, including zero; estimation is a
fallback, never silently substituted — the existing doc comment stands). The
**rate lookup order** changes:

1. **Pi catalog** (new, first): `split_pi_spec(model_spec)` → `find()` →
   `(input, output)` rates; cache-read discount = `costRead / costInput` when
   both are known (clamped to `[0, 1]`), else `0.0` (today's no-caching
   default — never *over*-estimate).
2. **Static minimal fallback** (unchanged): `fallback_model_pricing_mtok` for
   `claude:`/`codex:` native specs. This satisfies G4 — native CLI routes keep
   today's exact handling; the catalog is Pi-spec-only.
3. **Else 0.0** (unchanged): unknown model, no rates ⇒ estimate 0. R1 is
   removed from the chain entirely (`infer_model_pricing` is deleted).

Effect: `lunaroute/*`, `zai/*`, and any future `pi:` provider stop showing
$0.00 whenever the catalog carries rates (the milestone's lunaroute fixture
test asserts non-zero cost from catalog rates). Where Pi's store itself
reports 0 (e.g. the current lunaroute gateway entries), the estimate stays 0 —
that is now **Pi's** reported truth, not WG's ignorance.

`stream_event::pi_usage_cost` and the provider-reported-cost preference are
untouched: catalog rates only feed the *estimate* path.

### D4 — The daemon refresh path is deleted, not disabled

Remove from `src/commands/service/mod.rs`: `run_registry_refresh`,
`do_registry_refresh`, `record_registry_refresh_outcome`, `RegistryRefreshState`
(+ its persistence), `registry_refresh_has_selected_route`,
`REGISTRY_REFRESH_FAILURE_THRESHOLD`, `REGISTRY_REFRESH_COOLDOWN`, the timer
call site, and the `coordinator.registry_refresh_interval` config key (added to
the migrate deprecated-key list, §D5).

**Consumers of the refresh output, rewired in the same change:**

- `BenchmarkRegistry` / `.wg/model_benchmarks.json` **survives as a read-only
  static artifact** — the last manually-produced snapshot (`wg models fetch`
  stays, Wave 3, and remains an explicit human action). Readers
  (`model_benchmarks::BenchmarkRegistry::load`) are unchanged.
- `wg profile pi` auto-configuration (`auto_configure_dynamic`) currently
  **errors** ("Run `wg models fetch` first") when no snapshot exists. It gains
  a deterministic fallback: when no benchmark snapshot exists, rank candidates
  from **Pi-catalog metadata** (context window, cache-read economics, cost —
  the cheap-to-compute subset; benchmark/quality indices stay snapshot-only
  enrichment). No silent behavior change when a snapshot *does* exist — the
  snapshot still wins.
- Triage ranking (`commands/service/triage.rs`) reads the snapshot; unchanged
  (it already tolerates absence).

After this change the **only** remaining key-requesting catalog fetches are
explicit, human-invoked CLI verbs (`wg models search/remote/fetch`,
`wg model-scout`) — no daemon path. Wave 3 (§7) retires or repoints those.

### D5 — `[[model_registry]]` config deprecation via the standard migrate path

- `Config.model_registry` / `ModelRegistryEntry` remain **parseable but inert**
  during the deprecation window (serde keeps loading them; nothing reads them
  for behavior). This mirrors the bare-provider-prefix deprecation pattern
  (loud, never silently re-routed).
- `wg migrate config` gains the transition: the canonicalization pipeline
  (`src/commands/migrate.rs`, the same machinery that strips
  `[agent].executor` and deprecated top-level keys) removes the
  `model_registry` key/tables, reports `removed deprecated key:
  model_registry`, writes the pre-migration backup, and is `--dry-run`-able.
- `wg config lint` flags any remaining `[[model_registry]]` as deprecated with
  the migrate command as the fix. `wg config lint` is clean after migration.
- `wg config model add/remove/list`: `add`/`remove` become deprecated writers
  (warn + still write, for one release) and `list` (`show_registry`)
  **repoints to the Pi catalog** — a read-only rendering of what Pi actually
  knows, since the entries no longer affect anything. Full removal is Wave 2.
- Consumers that must be rewired **in this same change** (no dangling
  half-state):
  - `resolve_model_via_registry` (spawn): bare-alias → model/endpoint
    resolution via R1 goes away; alias resolution now requires a handler-first
    full spec (the post-handler-first norm). The registry-lookup branch is
    deleted; provider-prefix handling and `claude_cli_model_arg` expansion are
    preserved byte-for-byte.
  - `merge_resolution`: the `registry.map(|r| r.tier)` /
    `descriptors contain "strong"` fallback is deleted; the merger route's
    strength comes **only** from the role's explicit `tier` (or the snapshot's
    existing strong/premium assertion). Absent an explicit tier it fails with
    a precise `MR_ROUTE_WEAK: set tier on the merger role` — fail-closed, never
    inferred from a dead registry.
  - `wg setup` wizard / `wg init` route inheritance: stop writing
    `[[model_registry]]` blocks and `auto_map_tiers` from registry entries;
    the wizard lists routes from the Pi catalog (read-only) plus the static
    claude/codex aliases. Setup continues to write `[tiers]` /
    `[models.<role>]` — the actual authority.
  - `project_config.rs` known-key lists: `model_registry` moves to the
    deprecated list (accepted-but-flagged), matching the migrate surface.
- Config lint Rule 3 ("Add a [[model_registry]] entry…") is deleted — it told
  users to fix a system we are removing. Rule 4 (entry's `model` field lacks
  `/`) dies with the deprecated-key flag.

### D6 — `wg config --models` display

`show_model_routing` already renders the route plane (per-role routes,
handler, provenance, revision) and is Pi-centric — it stays. Changes:

- Delete the dead `show_model_routing_legacy` (registered as `#[allow(dead_code)]`).
- Add one catalog-backed line block: for each resolved `pi:` route, the
  catalog's context window and rates (or `catalog: no entry`) — so the display
  shows what Pi knows about the models actually routed, replacing the
  registry-data rendering `wg config --models` historically surfaced. Rendered
  from `PiCatalog` only; no registry read.

### D7 — Keep the `claude:` / `codex:` native path minimal (explicitly)

The design deliberately does **not** route native CLI models through the
catalog: `claude:opus` etc. keep `fallback_model_pricing_mtok` and the CLI's
own auth/discovery. Rationale: the CLI handlers own their model identity
(self-authenticating, alias-expanding); injecting a catalog between them adds
a failure mode with no payoff. If a native route needs pricing later, the
catalog is an additive lookup, not a structural one.

### D8 — What stays in Milestone 1 (and why it is safe to stay)

- `wg models search/remote/fetch` and `wg model-scout` keep their OpenRouter
  fetches: explicit human verbs, not daemon code; G1 is about the daemon.
  They are scheduled for Wave 3 (§7).
- `src/models.rs::ModelRegistry` (`.wg/models.yaml`) is untouched in
  Milestone 1 — no consumer of it is being removed, so removing it would
  create exactly the dangling half-state this design forbids. Wave 2.
- `model_benchmarks.rs` stays as the snapshot format + ranking library
  (feed-forward for the Pi-catalog fallback in D4). Wave 3 evaluates folding
  it away.

---

## 4. Milestone 1 scope (implement-retire-the)

Ordered to keep the tree green at every step:

1. **Add `src/pi_catalog.rs`** with the fixture-driven unit tests (§6). No
   consumers yet — additive, inert.
2. **Rewire cost estimation** (D3): `estimate_agent_cost_usd` consults the
   catalog first; delete `infer_model_pricing` (R1 read). Update the existing
   graph token-usage tests.
3. **Delete the daemon refresh path** (D4): service/mod.rs refresh machinery +
   `registry_refresh_interval` handling; `auto_configure_dynamic` Pi-catalog
   fallback; migrate list gains the config key.
4. **Deprecate `[[model_registry]]`** (D5): migrate transition, lint flag,
   spawn/merge/setup/init rewires, `wg config model list` repoint.
5. **`wg config --models`** (D6): catalog-backed block, delete legacy fn.
6. **Tests + checks** (§6): `cargo test --lib`, `cargo fmt --check`,
   `cargo clippy`; smoke scenarios unaffected (run the completion trio via the
   harness bridge if any spawn/config path changed).

Every removal above lands with its consumer rewire in the same change.

---

## 5. Compatibility & migration

- **Config:** old configs with `[[model_registry]]` keep loading (inert);
  `wg migrate config` (idempotent, backed-up, dry-run-able) removes them;
  lint flags stragglers. No config is rewritten implicitly on load — only via
  the explicit migrate verb, matching the established deprecation pattern.
- **Accounting:** for models the old path priced via R1, the new path prices
  via the catalog (superset) or the static fallback (claude/codex). The only
  regressions are models that existed *only* in a hand-written R1 entry and
  are absent from Pi's store — for those the estimate becomes 0 (unknown),
  which is the documented fail-open-of-accounting posture: estimates are
  never presented as provider truth.
- **Daemon:** the refresh loop's absence removes background error/cooldown
  noise; `registry_refresh_interval: 0` configs (already the off state) are
  semantically identical post-migration.
- **No compat-const impact:** nothing here touches WG-Fed / WG-Review /
  WG-Exec or any `WG_*_COMPAT_VERSION` surface.

---

## 6. Validation (acceptance for Milestone 1)

- [ ] Registry-refresh path gone; a source scan of `src/commands/service/`
      finds no `fetch_openrouter_models_blocking` / API-key resolution on any
      daemon path; no daemon code requests provider API keys for catalog
      refresh
- [ ] Cost estimation reads Pi-catalog rates: unit test with a **lunaroute
      fixture** `models-store.json` (non-zero rates) shows a non-zero
      estimate; unknown-model and missing-file cases still estimate 0 without
      erroring
- [ ] `wg migrate config` handles legacy `[[model_registry]]` cleanly
      (removed + backed up + reported); `wg config lint` clean after
      migration; dry-run makes no changes
- [ ] Catalog reader unit tests: fixture-driven `find` / `split_pi_spec` /
      unknown-vs-free sentinel handling / empty-on-garbage behavior
- [ ] Spawn alias resolution and merge-resolution tests updated for the
      removed registry fallbacks (fail-closed paths asserted)
- [ ] `cargo test --lib` green; `cargo fmt --check` + `cargo clippy` clean;
      worktree clean

---

## 7. Follow-up waves (explicitly out of Milestone 1)

- **Wave 2 — retire R2 (`.wg/models.yaml`)**: delete `src/models.rs`
  registry, `wg models list/add/set-default/init`; rewire
  `supports_tool_use` callers (tui_nex/native_exec/nex_runtime) to the
  Pi-catalog `compat` surface (default-true preserved); rewire
  `load_model_choices` consumers.
- **Wave 3 — retire R4**: `wg models search/remote/fetch` and
  `wg model-scout` either retire or repoint to the catalog (the scout's
  value-aware selection logic could run over `PiCatalog` data instead of a
  live OpenRouter fetch — no key, no network); fold `model_benchmarks.json`
  away once the Pi-catalog ranking fallback (D4) has soaked.
- **Wave 4 — sweep**: delete the inert `ModelRegistryEntry` struct + serde
  shim, `project_config` deprecated-key tolerance, and any remaining
  registry-shaped dead code.

Each wave is its own task graph node depending on the previous one.

---

## 8. Risks

| Risk | Mitigation |
|------|------------|
| `models-store.json` absent (fresh install, CI, non-Pi user) | Empty catalog; accounting falls to static/0 exactly as today for unknown models; interactive surfaces warn once. No daemon behavior depends on the file. |
| Pi schema drift (Pi release adds/renames fields) | Defensive parse: unknown fields ignored, per-entry skip, whole-file failure ⇒ empty. The reader's tests pin only fields we consume. |
| Costs in the store are wrong/stale | That is now Pi's problem by construction (D1); WG reports what Pi believes, and provider-reported per-turn cost (when present) still overrides estimates. |
| Users relied on R1 endpoints/tier side-band | Migration is loud (lint + migrate output); spawn route authority was already handler-first; merge-resolution fails closed with an actionable message instead of silently downgrading. |
| Milestone sprawl | §4 fixes the order; §7 quarantines R2/R4 so the first milestone stays a reviewable diff. |
