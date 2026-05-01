# Canonical wg config — what to keep, what to drop, what's stale

**Audience:** Anyone editing `~/.wg/config.toml` (global) or `./.wg/config.toml` (project-local) and wondering "is this key still real?"

**Audit date:** 2026-04-27 (against worktree `agent-783`, branch `wg/agent-783/research-audit-wg`).

**TL;DR:** A typical user's global config should fit on one screen. Most of what's currently in `~/.wg/config.toml` (and the project-local override in `~/workgraph/.wg/config.toml`) is restated defaults that the code already provides. Trim to the keys you actually need to differ from the built-ins. Two specific stalenesses to fix today:

1. `models.default.model = "openrouter:anthropic/claude-sonnet-4"` in global is **stale** — it pins eval/assign roles' fallback to a non-existent endpoint and overrides cleaner per-route defaults.
2. The local config has `chat_agent = true` and `max_chats = 4`, but the code only knows `coordinator_agent` and `max_coordinators` (no aliases) — those keys are **silently dropped** on load.

---

## 1. Layered config resolution

### File precedence (`src/config.rs:3749-3802`, `src/main.rs:55-102`)

The graph directory is resolved first, **then** config is loaded:

| step | what is read | from |
|------|-------------|------|
| graph-dir resolution | `--dir` → `WG_DIR` → walk-up from cwd for `.wg`/`.workgraph` → `~/.wg`/`~/.workgraph` → default `./.wg` | `src/main.rs:55-102` |
| global config | `~/.wg/config.toml` (note: **hardcoded `.workgraph`**, not `.wg`) | `src/config.rs:3669-3677` (`Config::global_dir`) |
| project config | `<workgraph_dir>/config.toml` (whichever name resolved above) | `src/config.rs:3749-3754` (`Config::load_merged`) |
| matrix creds | `~/.config/workgraph/matrix.toml` (separate file, not merged into Config) | `src/config.rs:3367-3370` |

**Stale alert:** `Config::global_dir()` joins `.workgraph` literally; `resolve_workgraph_dir()` in `main.rs:53` prefers `.wg`. On Erik's system the two are hardlinked and that masks the inconsistency. New users running `wg init --global` get `~/.wg/config.toml` (per `main.rs:710-712`) — but `Config::load_global()` will then try to read `~/.wg/config.toml` and silently get `None`. **Fix candidate:** make `Config::global_dir()` use the same resolver order as `main.rs::resolve_workgraph_dir`.

### Per-key precedence

For any single setting, resolution flows top-down (highest precedence first):

1. **Per-task field** (`task.model`, `task.exec`, `task.tags`, `task.context_scope`) — set on the graph entry itself.
2. **CLI flag** for one-shot overrides — e.g. `wg add ... --model claude:opus`, `wg service start --max-agents 4`. Most wg subcommands accept `--model`/`--endpoint`/etc. (see `src/cli.rs`).
3. **Tag routing** — `[[tag_routing]]` rules match `task.tags` and provide a model+executor when `task.model` is unset (`src/config.rs:1388-1395`).
4. **Project-local config** — `<workgraph_dir>/config.toml`.
5. **Global config** — `~/.wg/config.toml` (deep-merged: project keys override globally; for `[[llm_endpoints.endpoints]]` see special opt-in below).
6. **Profile defaults** — when `profile = "anthropic"` (etc.) is set, that profile supplies tier defaults (`src/config.rs:2078-2092`).
7. **Hardcoded defaults** — `default_*()` functions and `Default for *Config` impls (`src/config.rs:184-…`, scattered).
8. **Built-in registry** — `Config::builtin_registry()` provides Anthropic Claude entries when `[[model_registry]]` is empty (`src/config.rs:1964-2053`).

### Special merge rules

- **Endpoint inheritance is opt-out** (`src/config.rs:3499-3523` / `apply_endpoint_inheritance_policy`). A local config's `[[llm_endpoints.endpoints]]` array **fully replaces** the global list. Set `[llm_endpoints] inherit_global = true` in local to get the legacy "global cascades into local" behavior.
- **Models-default agent override** (`strip_global_only_model_roles`, `src/config.rs:3546-3599`): when local config sets `agent.model`, any `[models.<role>].model` that exists only in global (not overridden locally) is stripped, so a per-project model choice doesn't get clobbered by global eval/assign overrides.
- **Legacy section rename** (`LEGACY_SECTION_ALIASES`, `src/config.rs:3450`): `[coordinator]` is migrated to `[dispatcher]` before merge, with a one-shot stderr warning per file. Both load — canonical wins on conflict.
- **Unknown keys are silently dropped.** No `#[serde(deny_unknown_fields)]` anywhere (verified by grep). Misspelled / renamed keys (e.g. `chat_agent` instead of `coordinator_agent`) load successfully and have zero effect. **This is the #1 footgun.**

### Resolution cascades for model selection

The cascade `Config::resolve_model_for_role` (`src/config.rs:2172-2336`) is more nuanced than "merged config wins":

```
1. [models.<role>].model            (explicit role override)
2. [models.<role>].tier             (role tier override)
3. role.default_tier() → [tiers]    (Fast/Standard/Premium → tier ID)
4. [models.default].model           (global cross-role default)
5. agent.model                      (final fallback)
```

Provider cascades through `[models.<role>].provider → [models.default].provider → coordinator.model prefix → agent.model prefix`.

When `agent.model` is set in **local** config (not global), step 3 is *skipped for `task_agent`* so a per-project `--model` choice routes worker agents directly there. Metacognition roles (assigner/compactor/etc.) still flow through tiers so they stay cheap (`src/config.rs:2271`).

---

## 2. Full key inventory

For each section: source-of-truth code line, default, and scope (G = belongs in global, P = project-local, B = both, N = neither / experimental / deprecated).

### `[agent]` — per-task agent defaults (`src/config.rs:2706-2737`)

| key | what it does | code | default | scope | status |
|-----|--------------|------|---------|-------|--------|
| `model` | Per-task default model. provider:model spec required by strict validator. Fallback when role cascade hits step 5. | `src/config.rs:2716, 3311` | `claude:opus` | **G** (project may override) | current |
| `executor` | Handler kind (claude/codex/native/shell). | `src/config.rs:2712` | `claude` | — | **deprecated** — derived from `model` provider prefix; warning emitted (`deprecated_executor_warnings_for_toml`, `src/config.rs:1767`). Skipped on serialize when default. |
| `interval` | Sleep interval between agent iterations (s). | `src/config.rs:2720, 3315` | `10` | G | current |
| `max_tasks` | Hard cap on tasks per agent run. | `src/config.rs:2724` | unlimited | N (rarely changed) | current |
| `heartbeat_timeout` | Minutes before agent declared dead. | `src/config.rs:2728, 3319` | `5` | G | current |
| `reaper_grace_seconds` | Seconds before reaper acts on dead PID (race-condition shield). | `src/config.rs:2735, 3323` | `30` | G | current |

### `[dispatcher]` (canonical) / `[coordinator]` (legacy alias) — daemon settings (`src/config.rs:2740-2999`)

The whole table renames `[coordinator]` → `[dispatcher]` automatically (`LEGACY_SECTION_ALIASES`). New configs should write `[dispatcher]`; old configs continue to load with a one-shot warning.

| key | what it does | code | default | scope | status |
|-----|--------------|------|---------|-------|--------|
| `max_agents` | Concurrent worker agent cap. | `:2743, 3064` | `8` | B | current |
| `interval` | Standalone-coordinator-cmd poll interval (s). | `:2747, 3068` | `30` | G | current |
| `poll_interval` (alias `safety_interval`) | Daemon safety-timer interval (s). With graph-fs watching + IPC kick, this is just the safety net for missed events. | `:2764, 3080` | `5` | G | current — `poll_interval` deprecated in favor of `safety_interval` (alias still works; `detect_deprecated_keys` warns, `src/config.rs:3624`) |
| `graph_watch_enabled` | Use `notify` watcher on graph.jsonl as primary trigger. | `:2774, 3088` | `true` | G | current |
| `graph_watch_debounce_ms` | Debounce window for graph fs watcher. | `:2783, 3092` | `100` | G | current |
| `executor` | Executor for spawned agents (None = derived from `model`). | `:2789, 3179` | `None` | — | **deprecated** — see `agent.executor` |
| `model` | Worker default model. Overrides `agent.model`. CLI `--model` overrides this. | `:2795, 3181` | `None` | B | current |
| `provider` | Pre-strip-format provider field. Never written, only read for back-compat. | `:2799` | `None` | — | **deprecated** — embedded in `model` prefix |
| `default_context_scope` | clean / task / graph / full. | `:2804` | `None` (= `task` per dispatcher logic) | B | current |
| `agent_timeout` | Hard timeout on spawned agent (e.g. `30m`, `1h`). Empty disables. | `:2810, 3164` | `"30m"` | G | current |
| `settling_delay_ms` | Wait after GraphChanged before dispatching (settles burst adds). | `:2817, 3072` | `2000` | G | current |
| `coordinator_agent` | Spawn the persistent chat LLM session for the daemon. | `:2825, 3076` | `true` | G | current. **NOTE: there is NO `chat_agent` alias.** |
| `compactor_interval` | (no-op) | `:2832, 3096` | `5` | — | **deprecated (retired-compact-archive)** — graph-cycle compactor removed; warns if non-default |
| `compactor_ops_threshold` | (no-op) | `:2836, 3100` | `100` | — | **deprecated** |
| `compaction_token_threshold` | (no-op) | `:2840, 3104` | `100000` | — | **deprecated** |
| `compaction_threshold_ratio` | (no-op) | `:2844, 3108` | `0.8` | — | **deprecated** |
| `eval_frequency` | Coordinator turn-eval cadence. `every_5` etc. | `:2849, 3112` | `"every_5"` | G | current |
| `worktree_isolation` | Isolate each agent in its own git worktree. | `:2855, 3116` | `true` | G | current |
| `max_coordinators` | Concurrent chat (LLM) sessions cap. | `:2860, 3120` | `16` | G | current. **NOTE: there is NO `max_chats` alias.** |
| `archive_retention_days` | Days before done/abandoned tasks archived (0 disables). | `:2867, 3124` | `7` | G | current |
| `registry_refresh_interval` | Seconds between OpenRouter registry refresh (0 disables). | `:2875, 3160` | `86400` | G | current |
| `verify_mode` | `inline` or `separate` for legacy verify-cmd tasks. | `:2883, 3128` | `"inline"` | — | **soon-to-deprecate** — replaced by `## Validation` section in task descriptions; agency evaluator reads it instead |
| `verify_autospawn_enabled` | Master switch for `.verify-*` shadow tasks. | `:2897` | `false` | — | **deprecated** (2026-04-17) — runaway meta-task cascades; replacement is `.evaluate-*` + `wg rescue` |
| `max_verify_failures` (alias `max_eval_rescues`) | Verify circuit breaker / cascade-rescue cap. | `:2909, 3132` | `3` | G | current |
| `verify_default_timeout` | Default per-task verify timeout. | `:2913, 3136` | `"900s"` | G | current |
| `max_concurrent_verifies` | Cap on parallel verify processes. | `:2917, 3140` | `2` | G | current |
| `verify_triage_enabled` | LLM-driven triage instead of hard verify timeout. | `:2921, 3144` | `false` | G | experimental |
| `verify_progress_timeout` | Stuck-process detection threshold. | `:2925, 3148` | `"300s"` | G | current |
| `max_spawn_failures` | Spawn-fail circuit breaker. | `:2933, 3152` | `5` | G | current |
| `max_escalation_depth` | Max tier escalation on retry. | `:2942, 3156` | `3` | G | current |
| `auto_test_discovery` | Pre-spawn test scanning + injection. | `:2947, 3048` | `false` | G | experimental |
| `scoped_verify_enabled` | Run only relevant tests on verify (faster). | `:2955, 3052` | `true` | G | current |
| `on_provider_failure` | `pause` / `fallback` / `continue`. | `:2962, 3056` | `"pause"` | G | current |
| `provider_failure_threshold` | Consecutive fatal-provider errors before pause. | `:2969, 3060` | `3` | G | current |
| `provider_failure_cooldown` | Auto-resume cooldown (`5m`, `1h`; empty = manual). | `:2975` | `""` | G | current |
| `max_incomplete_retries` | Retries on incomplete-marked task. | `:2985, 3040` | `3` | G | current |
| `incomplete_retry_delay` | Cooldown before respawn (`30s`). | `:2991, 3044` | `"30s"` | G | current |
| `escalate_on_retry` | Bump quality tier on retry. | `:2997` | `false` | G | current |

### `[dispatcher.resource_management]` — worktree cleanup (`src/config.rs:3002-3038`)

All G-scope, all current. Defaults: `cleanup_verification = true`, `recovery_branch_max_age = 604800` (7d), `recovery_branch_max_count = 10`, `cleanup_job_queue = true`, `cleanup_queue_size = 50`, `recovery_prune_interval = 3600` (1h). Code: `src/config.rs:3204-3239`.

### `[project]` — project metadata (`src/config.rs:3287-3301`)

| key | what it does | default | scope |
|-----|--------------|---------|-------|
| `name` | Display name. | `None` | **P** only |
| `description` | One-line. | `None` | **P** only |
| `default_skills` | Skills attached to new actors. | `[]` | **P** only |

### `[help]` — help display (`src/config.rs:467-484`)

| key | default | scope |
|-----|---------|-------|
| `ordering` | `"usage"` (other: `alphabetical`, `curated`) | G |

### `[agency]` — evolutionary identity layer (`src/config.rs:2429-2703`)

A large block. Only the keys most likely to matter to set are listed; full schema is in code.

| key | code | default | scope | notes |
|-----|------|---------|-------|-------|
| `auto_evaluate` | `:2433` | `true` | B | post-task evaluation |
| `auto_assign` | `:2437` | `true` | B | auto-assign agent identity |
| `assigner_agent`, `evaluator_agent`, `evolver_agent`, `creator_agent`, `placer_agent` | `:2441-2458` | `None` | B (per project; identity hashes are content-addressable) | content-hash refs into `.wg/agency/` |
| `auto_place` | `:2464` | `false` | B | placement during assignment |
| `auto_create` | `:2469` | `false` | B | auto-invoke creator agent |
| `auto_create_threshold` | `:2474` | `20` | B | |
| `auto_triage`, `triage_timeout`, `triage_max_log_bytes` | `:2483-2492` | `false`, `None`, `None` | B | dead-agent triage |
| `exploration_interval` | `:2496` | `20` | B | learning-mode interval |
| `cache_population_threshold` | `:2502` | `0.8` | B | |
| `ucb_exploration_constant` | `:2508` | `√2 ≈ 1.414` | B | |
| `novelty_bonus_multiplier` | `:2513` | `1.5` | B | |
| `bizarre_ideation_interval` | `:2518` | `10` | B | |
| `auto_assign_grace_seconds` | `:2524` | `10` | B | |
| `eval_gate_threshold` | `:2535` | `Some(0.7)` | B | |
| `eval_gate_all` | `:2540` | `false` | B | apply gate to all evals |
| `auto_rescue_on_eval_fail` | `:2552` | `true` | B | |
| `flip_enabled` | `:2557` | `false` (built-in) / `true` (in current configs) | B | FLIP scoring |
| `flip_inference_model`, `flip_comparison_model` | `:2561-2566` | `None` | B | |
| `flip_verification_threshold` | `:2572` | `None` | B | **deprecated** (2026-04-17) — autospawn cascades. Set to re-enable old behavior. |
| `auto_evolve` | `:2578` | `false` | B | |
| `evolution_interval` | `:2582` | `7200` (2h) | B | |
| `evolution_threshold` | `:2587` | `10` | B | |
| `evolution_budget` | `:2592` | `5` | B | |
| `evolution_reactive_threshold` | `:2598` | `0.4` | B | |
| `agency_server_url`, `agency_token_path`, `agency_project_id` | `:2603-2616` | `None` | G | external Agency server bindings |
| `assignment_source` | `:2611` | `None` | G | |
| `upstream_url` | `:2620` | `None` | G | bureau CSV import URL |
| `evaluator_model` | `:2627` | `None` | B | tier-pin override |
| `default_validation_mode` | `:2632` | `None` | B | none/integrated/external/llm |
| `gate_uncertain_policy` | `:2640` | `"escalate"` | B | escalate/retry/fail-closed |
| `gate_max_attempts` | `:2646` | `2` | B | |
| `gate_confidence_threshold` | `:2652` | `0.7` | B | |

### `[log]`, `[replay]`, `[guardrails]`, `[viz]` — small tables

| section | key | code | default | scope |
|---------|-----|------|---------|-------|
| `[log]` | `rotation_threshold` | `:489, 494` | `10485760` (10MB) | G |
| `[replay]` | `keep_done_threshold` | `:510, 518` | `0.9` | B |
| `[replay]` | `snapshot_agent_output` | `:514` | `false` | B |
| `[guardrails]` | `max_child_tasks_per_agent` | `:537, 558` | `10` | G |
| `[guardrails]` | `max_task_depth` | `:542, 562` | `8` | G |
| `[guardrails]` | `max_triage_attempts` | `:547, 566` | `3` | G |
| `[guardrails]` | `decomp_guidance` | `:554, 570` | `true` | G |
| `[viz]` | `edge_color` | `:589, 596` | `"gray"` | G |
| `[viz]` | `animations` | `:592, 600` | `"normal"` | G |

### `[tui]` — TUI settings (`src/config.rs:614-730`)

All G-scope (cosmetic / per-user). Defaults at `:670-705`:

`mouse_mode = None`, `default_layout = "auto"`, `color_theme = "dark"`, `timestamp_format = "relative"`, `show_token_counts = true`, `message_name_threshold = 8`, `message_indent = 2`, `panel_ratio = 67`, `default_inspector_size = "2/3"`, `chat_history = true`, `chat_history_max = 1000`, `chat_page_size = 100`, `counters = "uptime,cumulative,active,compact"`, `show_system_tasks = false`, `show_running_system_tasks = false`, `show_keys = false`, `session_gap_minutes = 30`.

### `[chat]` — chat archive rotation (`src/config.rs:168-206`)

| key | code | default | scope |
|-----|------|---------|-------|
| `max_file_size` | `:172, 184` | `1048576` (1MB) | G |
| `max_messages` | `:175, 187` | `10000` | G |
| `retention_days` | `:178, 190` | `30` | G |
| `compact_threshold` | `:181, 193` | `50` | G |

### `[checkpoint]` — agent context preservation (`src/config.rs:957-1001`)

| key | code | default | scope |
|-----|------|---------|-------|
| `auto_interval_turns` | `:961, 976` | `15` | G |
| `auto_interval_mins` | `:964, 980` | `20` | G |
| `max_checkpoints` | `:968, 984` | `5` | G |
| `retry_context_tokens` | `:972, 988` | `2000` | G |

### `[[llm_endpoints.endpoints]]` — provider endpoints (`src/config.rs:733-953`)

Repeating array; one entry per endpoint. Per-entry fields: `name`, `provider` (default `"anthropic"`), `url`, `model`, `api_key`, `api_key_file` (with `~`/relative-path expansion), `api_key_env`, `is_default` (default `false`), `context_window`. Sibling: `[llm_endpoints] inherit_global = false` (default false; opt-in).

Scope: typically **G** for shared endpoints (the openrouter API key etc.); **P** for per-project oddballs (a local llama.cpp instance only this project hits).

### `[checkpoint]`, `[models.*]`, `[tiers]`, `[[model_registry]]`, `[[tag_routing]]`, `profile`

- **`profile`** (top-level, `src/config.rs:78`): name of an active provider profile (e.g. `"anthropic"`). When set, supplies tier defaults from `src/profile.rs`. Default `None`. Scope: G.
- **`[models.<role>]`** (`src/config.rs:1397-1413`): `provider` (deprecated — embed in model spec), `model` (provider:model spec), `tier` (Fast/Standard/Premium), `endpoint` (named endpoint override). Roles: `default`, `task_agent`, `evaluator`, `flip_inference`, `flip_comparison`, `assigner`, `evolver`, `verification`, `triage`, `creator`, `compactor`, `placer`, `chat_compactor`. Scope: B.
- **`[tiers]`** (`src/config.rs:1347-1358`): `fast`, `standard`, `premium` model IDs. Defaults via `Config::effective_tiers` cascade through profile then `claude:haiku/sonnet/opus`. Scope: G (set once for the user) or per-project when overriding.
- **`[[model_registry]]`** (`src/config.rs:1287-1324`): full per-model registry entry. Fields: `id`, `provider`, `model`, `tier`, `endpoint` (optional), `context_window`, `max_output_tokens`, cost fields, `prompt_caching`, descriptors. Built-in registry (`Config::builtin_registry`, `:1964-2053`) supplies Anthropic Claude entries. Scope: G unless project ships custom local-models metadata.
- **`[[tag_routing]]`** (`src/config.rs:1364-1380`): `tag`, `model`, `executor` (optional). Scope: P (project-specific tag taxonomies).

### `[openrouter]` — cost cap monitoring (`src/config.rs:362-464`)

| key | default | scope | notes |
|-----|---------|-------|-------|
| `cost_cap_global_usd`, `cost_cap_session_usd`, `cost_cap_task_usd` | `None` | B | optional caps |
| `cap_behavior` | `"escalate"` | G | fail/fallback/escalate/readonly |
| `fallback_model` | `None` | G | for `cap_behavior = fallback` |
| `key_status_check_interval_minutes` | `5` | G | |
| `warn_at_usage_percent` | `80` | G | |
| `cost_estimation_buffer` | `1.2` | G | |
| `enable_cache_tracking` | `true` | G | |
| `track_session_costs` | `true` | G | |
| `persist_cost_history` | `false` | G | |

### `[native_executor.*]` — native executor (`src/config.rs:209-360`)

| section | key | default | scope |
|---------|-----|---------|-------|
| `[native_executor.web]` | `search_api_key` | `None` | G (or env) |
| | `searxng_url` | `None` | G |
| | `fetch_max_chars` | `16000` | G |
| | `fetch_timeout_secs` | `30` | G |
| `[native_executor.background]` | `max_background_tasks` | `5` | G |
| | `background_timeout_secs` | `600` | G |
| `[native_executor.delegate]` | `delegate_max_turns` | `10` | G |
| | `delegate_model` | `""` (= same as parent) | G |
| `[native_executor.permissions]` | `deny_tools` | `[]` | P (project-specific safety) |

### `[mcp]` — MCP servers (`src/config.rs:144-165`)

`[[mcp.servers]]` array: `name`, `command`, `args`, `env`, `enabled` (default `true`). Scope: B — typically **P** (project-specific tools), but a personal `filesystem` server may live in **G**.

### Matrix credentials — separate file

Stored at `~/.config/workgraph/matrix.toml`, NOT in main config (`src/config.rs:3340-3421`). Keys: `homeserver_url`, `username`, `password`, `access_token`, `default_room`. Scope: G-only (it's user creds).

---

## 3. Stale-value audit

### Confirmed staleness in current `~/.wg/config.toml` (Erik's home)

| line | key | current value | fix |
|-----|-----|---------------|-----|
| 136 | `[models.default].model` | `"openrouter:anthropic/claude-sonnet-4"` | Either drop the section entirely (let tier cascade pick), or set to `"claude:opus"` or `"openrouter:anthropic/claude-sonnet-4-6"` (the actually-shipping model). Combined with the rest of the openrouter setup absent, this pins the global eval/assign cascade to a model not reachable by any configured endpoint. |
| 122-127 | `[[llm_endpoints.endpoints]]` openrouter | `is_default = true` | OK if user wants OpenRouter as default; but combined with `[tiers]` set to `claude:*` and `[models.default]` set to `openrouter:anthropic/claude-sonnet-4`, the wiring is internally inconsistent. Either (a) commit to OpenRouter (set tiers to `openrouter:anthropic/claude-haiku-4`/`sonnet-4`/`opus-4`), or (b) commit to `claude` CLI and remove the openrouter endpoint. |
| 144-147 | `[tiers]` | `fast = "claude:haiku"`, `standard = "claude:sonnet"`, `premium = "claude:opus"` | Match `~/.wg/config.toml` and the project setup: keep `claude:opus`/`claude:sonnet`/`claude:haiku`. **Current values are fine.** Keep these. |
| 149-162 | `[[model_registry]]` qwen3-coder-30b | `endpoint = "lambda01"` | Stale endpoint name (no `[[llm_endpoints.endpoints]]` named `lambda01` in the same file). Either add the endpoint or drop the registry entry. |
| 191 | `[mcp]` | empty | OK — empty section is harmless but produces a non-default output on `wg config show`. Drop the line. |

### Confirmed staleness in current local `.wg/config.toml` (this repo)

| line | key | current value | issue |
|-----|-----|---------------|-------|
| 14 | `chat_agent = true` | — | **silently ignored** — code only knows `coordinator_agent`. No alias. Either rename to `coordinator_agent` or remove (default is already `true`). |
| 21 | `max_chats = 4` | — | **silently ignored** — code only knows `max_coordinators`. Either rename to `max_coordinators` or remove. |
| 7-40 | `[dispatcher]` | most keys are restated defaults | drop everything that matches built-in defaults (see "Minimal project config" below). |
| 145 | `[tiers]` | empty (`[tiers]` header with no fields) | drop the section entirely — empty section serializes to nothing useful and a future `--tier` setter will see a section to fill. |

### Code-level "soon-to-deprecate" / no-op flags

Items listed as **deprecated** above. The big ones:

- `agent.executor`, `dispatcher.executor` — derived from model spec.
- `coordinator.compactor_*` and `coordinator.compaction_*` — graph-cycle compactor retired.
- `coordinator.verify_autospawn_enabled`, `agency.flip_verification_threshold` — replaced by `.evaluate-*` + `wg rescue`.
- `coordinator.verify_mode` — pre-Validation-section legacy.
- `dispatcher.poll_interval` — alias for `safety_interval` (canonical).

### Naming inconsistencies worth tracking

- **`global_dir()` returns `~/.workgraph`** even though `wg init --global` writes `~/.wg/config.toml`. Currently masked by hardlinks; will silently break for new users.
- **`coordinator_agent`/`max_coordinators`** in the schema vs **`chat_agent`/`max_chats`** that the user wrote in local config. One of:
    - add `serde(alias = "chat_agent")` so the rename works,
    - or document the canonical name and drop the legacy attempt from the project file.
  The recent "rename coordinator → chat in user-visible strings" work (`9f91c83d3`) renamed UI surfaces but left the config schema on `coordinator_agent`.

---

## 4. Minimal global config — `~/.wg/config.toml` (or `~/.wg/config.toml`)

For a "I use claude CLI for everything, opus is my workhorse" user, this is the entire config:

```toml
[agent]
model = "claude:opus"

[tiers]
fast = "claude:haiku"
standard = "claude:sonnet"
premium = "claude:opus"

[models.evaluator]
model = "claude:haiku"

[models.assigner]
model = "claude:haiku"
```

That's six lines of meaningful config. Everything else falls through to built-in defaults.

If you also use OpenRouter, add:

```toml
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
is_default = false   # set true if you want OpenRouter to be the routing default
```

If you regularly use TUI:

```toml
[tui]
counters = "uptime,cumulative,active,compact"
# only override defaults you actually disagree with
```

If you use Matrix/Telegram/etc. notifications, those live in their own files (`~/.config/workgraph/matrix.toml`) — don't pollute `config.toml` with them.

**What you should NOT keep in global** (these are restated built-in defaults):

- `agent.interval`, `agent.heartbeat_timeout`, `agent.reaper_grace_seconds`
- The entire `[dispatcher]` block unless you actually run with `max_agents != 8` or `agent_timeout != 30m`
- `[checkpoint]`, `[chat]`, `[guardrails]`, `[viz]`, `[log]`, `[replay]`, `[help]` unless tuned
- All `[agency]` keys other than the `*_agent` content hashes (those *do* belong here — they're per-user identity bindings)
- `[openrouter]` unless setting cost caps
- `[native_executor.*]` unless tuning fetch limits or denying specific tools

### Recommended minimal global config (concrete proposal for Erik)

```toml
# ~/.wg/config.toml

[agent]
model = "claude:opus"

[tiers]
fast = "claude:haiku"
standard = "claude:sonnet"
premium = "claude:opus"

[models.default]
model = "claude:opus"

[models.evaluator]
model = "claude:haiku"

[models.assigner]
model = "claude:haiku"

# Optional: enable agency identity bindings
[agency]
auto_evaluate = true
auto_assign = true
assigner_agent = "eea940a6f6be13d60578dee27be1f4bade4fcaab05bbbe54b9c5ef4b2d05eae0"
evaluator_agent = "3184716484e6f0ea08bb13539daf07686ee79d440505f1fdf2de0357707034c3"
evolver_agent = "5bdc329d9bc48f0630bcdf6f201528d18e2806a9d6b6fbdabca684e7bbe62c3e"
creator_agent = "ead7f53029b7d01980e12f8beb6ad13f6907750479eb2951dd75eb63951922b8"
flip_enabled = true

# Optional: only if openrouter is actually used somewhere
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
is_default = false  # was true — now false because tiers/models.default both use claude:*
```

That's about 30 meaningful lines, vs the current ~190-line global config. Result: easier to read, less surface area for stale defaults, every value present is an actual choice.

---

## 5. Minimal project config — `<project>/.wg/config.toml`

For most projects: **empty file or no file at all**. Inheritance from global handles everything.

Real reasons to write a project-local config:

- **Override the agent model** for this codebase (e.g. larger model for harder code):
  ```toml
  [agent]
  model = "claude:opus"
  ```
- **Shadow the global default endpoint** to force fallback (e.g. workgraph repo wants `claude` CLI not OpenRouter):
  ```toml
  [[llm_endpoints.endpoints]]
  # Shadow global openrouter so claude_handler is used.
  name = "openrouter"
  provider = "openrouter"
  url = "https://openrouter.ai/api/v1"
  api_key_env = "OPENROUTER_API_KEY"
  is_default = false
  ```
- **Tag-based routing for parts of the graph:**
  ```toml
  [[tag_routing]]
  tag = "frontend"
  model = "codex:gpt-5-codex"

  [[tag_routing]]
  tag = "research"
  model = "claude:haiku"
  ```
- **Project-specific MCP servers:**
  ```toml
  [[mcp.servers]]
  name = "filesystem"
  command = "npx"
  args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
  ```
- **Tool denials for safety:**
  ```toml
  [native_executor.permissions]
  deny_tools = ["bash", "wg_done"]
  ```

### Recommended minimal project config (concrete proposal for the workgraph repo)

```toml
# .wg/config.toml — workgraph repo

[agent]
model = "claude:opus"

# Shadow global openrouter as not-default so this repo runs through claude CLI
# (claude_handler uses CLI auth — no API key needed).
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
is_default = false

[models.default]
model = "claude:opus"

[models.evaluator]
model = "claude:haiku"

[models.assigner]
model = "claude:haiku"
```

About 15 lines. Compare to the current 175-line file with `chat_agent`/`max_chats` typos and dozens of restated defaults.

---

## 6. Migration plan (no breaking changes)

Because we don't break compatibility:

1. **Unknown-key warnings.** Add an opt-in lint command (`wg config lint` or `wg config check`) that walks the merged config and warns about keys serde silently dropped. Easy because we already have raw `toml::Value` available. Without `deny_unknown_fields` (which would be too aggressive — would break `flip_*` extension keys etc.), a positive allowlist of known keys per section is the only way to catch `chat_agent` typos.
2. **Add a `[serde(alias = "chat_agent")]` on `coordinator_agent`** and `[serde(alias = "max_chats")]` on `max_coordinators`. Two lines of change. Local config keeps working; future renames don't silently drop.
3. **Fix `Config::global_dir()` to use the same resolver order as `main.rs`** — try `~/.wg` first, fall back to `~/.workgraph` for legacy. Avoids the silent-divergence bug for new users.
4. **`wg config trim`** — read merged config, compare against built-in defaults, write back only the differences (with comments preserved if possible). Equivalent to "manually clean my config." First-time users running `wg config init <route>` already get this cleanly; existing users are stuck with whatever their config has accumulated.
5. **Stale-default sweep**: replace `openrouter:anthropic/claude-sonnet-4` references in `[models.default]` and `[tiers]` with `claude:opus` (or whatever the user's actual default is). The route generators in `src/config_defaults.rs:199-201` still write `claude-sonnet-4`; if Anthropic has moved to `claude-sonnet-4-6` on OpenRouter that's a one-line fix in `openrouter_default_registry()`.

---

## 7. Quick reference — "where does this default come from?"

If `wg show <task>` or `wg config show` displays a value you didn't set, the cascade for that role in **decreasing priority**:

1. **Per-task `task.<field>`** — set on the graph entry (`graph.jsonl`).
2. **CLI flag** — `wg add ... --model X`, `wg service start --max-agents N`.
3. **Tag routing** — `[[tag_routing]]` rules with matching tag.
4. **Project `<workgraph_dir>/config.toml`**.
5. **Global `~/.wg/config.toml`** (deep-merged).
6. **Profile defaults** — when `profile = "..."` is set.
7. **Hardcoded `default_*()` functions** (`src/config.rs`).
8. **Built-in registry** — `Config::builtin_registry()` (Anthropic Claude haiku/sonnet/opus baked in).

Run `wg config show --source` (if implemented) or grep:
```bash
grep -n "<key>" ~/.wg/config.toml ./.wg/config.toml
```
to find which file actually set the surprising value.

---

## Appendix: cross-reference — from real-world config to source line

For each section in Erik's `~/.wg/config.toml` (4332 bytes, 192 lines, modified 2026-04-27 12:39):

| line | section | source-of-truth |
|------|---------|------------------|
| 1-5 | `[agent]` | `src/config.rs:2706-2737` |
| 7-42 | `[dispatcher]` (note: legacy alias) | `src/config.rs:2740-2999` |
| 44-50 | `[dispatcher.resource_management]` | `src/config.rs:3002-3038` |
| 52 | `[project]` | `src/config.rs:3287-3301` |
| 54-55 | `[help]` | `src/config.rs:467-484` |
| 57-85 | `[agency]` | `src/config.rs:2429-2703` |
| 87-88 | `[log]` | `src/config.rs:486-503` |
| 90-92 | `[replay]` | `src/config.rs:506-528` |
| 94-98 | `[guardrails]` | `src/config.rs:531-583` |
| 100-102 | `[viz]` | `src/config.rs:586-611` |
| 104-120 | `[tui]` | `src/config.rs:614-730` |
| 122-127 | `[[llm_endpoints.endpoints]]` | `src/config.rs:733-953` |
| 129-133 | `[checkpoint]` | `src/config.rs:957-1001` |
| 135-142 | `[models.*]` | `src/config.rs:1397-1500` |
| 144-147 | `[tiers]` | `src/config.rs:1347-1358` |
| 149-162 | `[[model_registry]]` | `src/config.rs:1287-1324` |
| 164-168 | `[chat]` | `src/config.rs:168-206` |
| 170-177 | `[openrouter]` | `src/config.rs:362-464` |
| 179-188 | `[native_executor.*]` | `src/config.rs:209-360` |
| 191 | `[mcp]` | `src/config.rs:144-165` |

Reviewer should be able to read this file, grep `~/.wg/config.toml`, and know exactly what to keep, what to delete, and what to fix.
