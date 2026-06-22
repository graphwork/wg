# Research: pi.dev model management & mid-chat model switching

**Task:** `pi-research-models` · **Date:** 2026-06-22 · **Status:** investigation only (no production code changes)

Downstream consumers: `pi-design-integration` (Design: pi.dev integration plan).

This note studies how **Pi Coding Agent** (`pi.dev`, npm `@earendil-works/pi-coding-agent`,
repo `earendil-works/pi`) represents models and lets a user switch model *mid-conversation*,
then maps those patterns onto WG's existing routing (handler-from-prefix, named profiles,
quality tiers) and sketches a concrete mid-chat model-switch path for WG's own chat agent.
It closes with feasibility notes for a pi-as-chat/TUI front-end over WG.

It builds on two prior reports — reuse, do not re-derive:

- `docs/reports/evaluate-pi-as-wg-executor.md` (`evaluate-pi-as`, 2026-06-15) — Pi as a WG
  executor: config surface, RPC/print/PTY smoke tests, `pi:<model>` handler sketch, no-go-for-now verdict.
- `docs/research/openrouter-cli-executor-scan.md` (`research-openrouter-cli-executors`, 2026-06-14) —
  broad CLI-executor scan; Pi rated **strongest embed fit** via `--rpc` JSON-over-stdio.

---

## 0. TL;DR

- **Pi switches model in-process, warm.** Mid-chat the model is a *mutable property of the
  live session*: TUI `/model` opens a picker (the `models.json` file reloads on each open, no
  restart); RPC sends `set_model {provider, modelId}` / `cycle_model`. The agent process stays
  alive, the conversation (the in-memory `AgentMessage` array + the session JSONL) is retained,
  and the **next turn** uses the new model. Model selection is **session-wide, not per-prompt** —
  individual `prompt` requests carry no model field.
- **WG switches model cold, by respawn.** WG's existing per-chat switch is
  `wg chat set-executor <id> [--executor …] [--model …]` (alias `wg chat switch`). It persists a
  `CoordinatorState.{executor,model}_override`, **SIGTERMs the live handler**, and the supervisor
  **respawns** a fresh handler that re-reads the shared `chat/<ref>/{inbox,outbox}.jsonl`. History
  survives because it is **on disk**, not in-process. Same end state as Pi (new model continues the
  conversation); different cost (cold respawn vs warm swap).
- **The two converge cleanly.** Pi's `set_model`/`cycle_model`/`get_available_models` RPC verbs
  are the warm-swap analogue of WG's cold `SetChatExecutor` IPC. Pi's `models.json` provider block
  is the analogue of a WG **named profile + endpoint**; Pi's `--model`/`set_model` is the analogue
  of WG's **model spec + handler-from-prefix**. Pi has no tier concept; WG's tiers
  (fast/standard/premium) are the closest mapping for Pi's per-task model choice.
- **Recommendation:** add a thin **`wg chat model <id> <spec>` / TUI `/model` picker** as the
  user-facing verb (it should resolve to the existing `SetChatExecutor` IPC, so no new switch
  engine is needed), and — separately, later — teach the **in-process `nex` handler** a warm
  per-turn re-resolve so the `nex` path can match Pi's zero-respawn swap. CLI handlers
  (`claude`/`codex`/`pi`) inherently need a respawn; that is acceptable and already works.
- **pi-as-chat/TUI front-end is feasible and is the place where "terminal takeover" is desired,
  not a bug.** A user-launched, interactive Pi TUI bound to a WG chat session (Pi `--session-id` =
  WG chat id) is a legitimate front-end; the constraint is that it must be an *explicitly attached*
  human surface, never WG's default unattended worker/handler contract.

---

## 1. How Pi represents and configures models/providers

Sources: `docs/reports/evaluate-pi-as-wg-executor.md` (local smoke tests, 2026-06-15) and Pi's
`models.md` / `rpc.md` docs (fetched 2026-06-22).

### 1.1 Static configuration (`models.json` + flags + env)

Pi resolves a model from three layers, most-specific first:

1. **CLI flags** — `--provider <name>`, `--model <pattern>`, `--api-key <key>`, plus
   `--list-models [filter]` to enumerate. `--model` matches against a model's `id` *or* its
   human `name` ("the configured `name` is used for model matching").
2. **Environment keys** — `OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
   `GEMINI_API_KEY`, … (listed in `pi --help`). These satisfy built-in providers without any file.
3. **`~/.pi/agent/models.json`** — custom providers/endpoints. Schema:

   **Provider block:**
   | field | meaning |
   |---|---|
   | `baseUrl` | API endpoint URL |
   | `api` | API shim: `openai-completions` (most compatible) · `openai-responses` · `anthropic-messages` · `google-generative-ai` |
   | `apiKey` | credential; value resolution: `!shell-command`, `$ENV_VAR`, or a literal |
   | `headers` | custom HTTP headers (same `!`/`$`/literal resolution) |
   | `authHeader` | bool — auto-add `Authorization: Bearer <apiKey>` |
   | `models[]` | array of model entries (below) |
   | `modelOverrides` | per-model patches for built-in models |
   | `compat` | provider-wide compatibility settings |

   **Model entry:**
   | field | meaning |
   |---|---|
   | `id` (req) | model identifier sent to the API |
   | `name` | human label, used for `--model` matching + UI |
   | `api` | override the provider's shim for this model |
   | `reasoning` | bool — extended thinking support |
   | `thinkingLevelMap` | maps `off/minimal/low/medium/high/xhigh` → provider value (or `null` = unsupported) |
   | `input` | `["text"]` or `["text","image"]` |
   | `contextWindow` | token window (default 128000) |
   | `maxTokens` | max output (default 16384) |
   | `cost` | `{input, output, cacheRead, cacheWrite}` per-million |
   | `compat` | per-model compat overrides |

   OpenRouter-style IDs work directly (smoke test listed `openrouter anthropic/claude-3.5-haiku`;
   a custom `openai-completions` provider accepted `anthropic/claude-3.5-haiku` as a model id).

### 1.2 Session model

Models and sessions are orthogonal: `--session`/`--session-id`/`--session-dir`/`--continue`/
`--resume`/`--no-session` control persistence (JSONL session files); the model is a separate axis
that can change *within* a session (next section). `get_state` reports the current `model`,
`sessionFile`, `sessionId`, `messageCount`, `isStreaming`.

---

## 2. How Pi switches model mid-chat (UX + mechanism)

This is the core question. Pi exposes the same capability through two surfaces.

### 2.1 Interactive TUI — `/model`

UX: the user types `/model` to open a model picker. Critically, **"the file reloads each time you
open `/model`. Edit during session; no restart needed."** So a user can hand-edit `models.json`
(add a provider, fix a key) and pick it up live, then select a model — all without restarting the
agent. Thinking depth is a separate live control (`thinkingLevelMap` / cycle).

### 2.2 RPC mode (`--mode rpc`) — `set_model` / `cycle_model`

UX-for-embedders: a host program writes newline-delimited JSON to Pi's stdin. The model verbs:

| RPC request | fields | effect |
|---|---|---|
| `set_model` | `{"type":"set_model","provider":"anthropic","modelId":"claude-sonnet-4-20250514"}` | switch to a specific model; response returns the full `Model` object, `success:true` |
| `cycle_model` | `{"type":"cycle_model"}` | rotate to next configured model; returns `model`, `thinkingLevel`, `isScoped` |
| `get_available_models` | `{"type":"get_available_models"}` | list configured models |
| `set_thinking_level` | level ∈ `off/minimal/low/medium/high/xhigh` | reasoning depth |
| `cycle_thinking_level` | — | rotate reasoning depth |

There is **no `set_provider`** — provider is carried alongside `modelId` in `set_model`. And
**individual `prompt` requests do not include a model field** — "model selection is session-wide."

### 2.3 The mechanism (why it's "warm")

The defining property: **the model swap does not tear down the session or the process.**

- State persists across requests *within the single RPC process instance*. `get_messages` returns
  the live `AgentMessage[]` (roles `user`/`assistant`/`toolResult`/`bashExecution`, all timestamped);
  sessions persist as JSONL and can be hot-swapped with `switch_session`.
- `set_model` mutates the session's current-model pointer; the *next* `agent_start → … → agent_end`
  cycle uses the new model against the **same** retained history.
- So switching `opus → haiku → gpt-5` mid-conversation costs one in-process pointer update plus a
  possible `models.json` reload — **no respawn, no replay, no cold start.**

This is the pattern WG does not yet have for its own chat agent (WG's equivalent is a respawn — §4).

### 2.4 Other session verbs worth noting (for the front-end design)

`new_session`, `switch_session`, `fork` / `clone` / `get_fork_messages` (branch a conversation),
`compact` / `set_auto_compaction`, `set_auto_retry` / `abort_retry`, `get_session_stats`
(tokens+cost), `export_html`, `steer` / `follow_up` (queue input during/after streaming),
`abort`. Streamed events: `agent_start/agent_end`, `turn_start/turn_end`,
`message_start/message_update/message_end` (deltas: `text_delta`, `thinking_delta`,
`toolcall_delta`), `tool_execution_*`, `queue_update`, `compaction_*`, `auto_retry_*`,
`extension_error`. These are the structured surface a WG front-end would render itself.

---

## 3. Pi → WG concept mapping

### 3.1 Concept mapping table

| Pi concept | What it is in Pi | Closest WG concept | WG location |
|---|---|---|---|
| `models.json` provider block (`baseUrl`,`api`,`apiKey`,`headers`,`authHeader`) | per-provider endpoint + wire shim + credential | **Named profile + `[[llm_endpoints.endpoints]]` + `wg secret` ref** | `src/profile/named.rs`, `src/profile/templates/*.toml`, `api_key_ref="keyring:…"` |
| Pi `api` shim (`openai-completions` / `anthropic-messages` / …) | wire protocol for a provider | **Handler wire protocol** (Anthropic vs OAI-compat), derived from prefix | `handler_for_model.rs:28` mapping table |
| `--provider <name>` + `--model <id\|name>` | provider + model selection at launch | **Model spec** `provider:model` (`claude:opus`, `nex:qwen3-coder`, `openrouter:vendor/model`) | `parse_model_spec`, `handler_for_model.rs:71` |
| Provider → built-in agent loop | which engine runs the model | **`ExecutorKind` / handler-from-prefix** (`claude`/`codex`/`native`(nex)/`opencode`/…) | `handler_for_model.rs:71`, `provider_to_executor` |
| Model entry `cost`, `contextWindow`, `reasoning`, `thinkingLevelMap` | per-model metadata | **`ModelRegistryEntry` + model registry / benchmarks** | `config.rs:1413` (`ModelRegistryEntry`), `src/models.rs`, `src/model_benchmarks.rs` |
| `thinkingLevel` (off…xhigh) + `set_thinking_level` | live reasoning depth | **nex thinking support** (no first-class tier knob yet) | nex thinking commits; no `set_thinking_level` IPC today |
| `--session-id` / session JSONL | conversation identity + history file | **WG chat id + `chat/<ref>/{inbox,outbox}.jsonl`** | `src/chat_id.rs`, `src/chat_sessions.rs`, `chat/coordinator-<N>/*.jsonl` |
| `set_model {provider, modelId}` (warm, in-process) | mid-chat model swap | **`SetChatExecutor` IPC** (cold, respawn) | `ipc.rs:602`, `handle_set_coordinator_executor` `ipc.rs:1909` |
| `cycle_model` | rotate among configured models | *(no WG analogue)* — would rotate over tiers or a profile's model list | candidate: `wg chat model --cycle` |
| `get_available_models` | enumerate configured models | **`wg models` / model registry list** | `src/models.rs:363` (`list(tier)`) |
| TUI `/model` picker (file reloads live) | interactive switch UX | *(no WG analogue yet)* — TUI has a model field at **chat create**, not mid-chat | `src/tui/viz_viewer/chat_*` (create-time only) |
| (no Pi concept) | — | **Quality tier (fast/standard/premium)** | `config.rs:1362`; `default_alias` fast=haiku, standard/premium=opus (`config.rs:1383`) |
| (no Pi concept) | — | **Profile-as-swap global flip + daemon hot-reload** | `profile_cmd.rs:720` `use_profile`, `:826` `trigger_daemon_reload` → IPC `Reconfigure{profile}` |

### 3.2 The three WG "switch" scopes (and where Pi's switch lands)

WG already has model-switching at **three different scopes**; Pi's `/model` lands squarely in the
third (per-chat), and that is the one WG should make first-class for its chat agent.

| Scope | WG mechanism | Granularity | When it takes effect |
|---|---|---|---|
| **Global default** | `wg profile use <name>[:<model>]` → writes `~/.wg/active-profile`, applies profile-as-global-config, IPC `Reconfigure{profile}` | every *future* worker the daemon spawns | next spawn (already-running agents keep their model) — `profile_cmd.rs:720` |
| **Per-role / per-tier** | `[models.<role>]`, `[tiers]` (fast/standard/premium); `resolve_model_for_role` cascade | per dispatch role / quality tier | at each task spawn — `config.rs:2478` |
| **Per-chat (the Pi analogue)** | `wg chat set-executor <id> --model … --executor …` (alias `switch`) → `SetChatExecutor` IPC | one specific live chat agent | on handler respawn — `ipc.rs:602`, `cli.rs:4917` |

Key contrast: **profile flip is "next worker"; Pi `/model` is "this conversation, now."** WG's
per-chat `set-executor` is the right scope but pays a respawn; Pi pays nothing. §4 details the WG
path and §5 the convergence.

---

## 4. WG's existing per-chat switch path (what we already have)

This is the per-message / per-turn switch path the task asks us to identify. It exists today and
is the foundation to build on — WG does **not** need a new switch engine, only a friendlier verb
and (optionally) a warm fast-path.

**User verb.** `wg chat set-executor <id> [--executor <e>] [--model <spec>]` (alias
`wg chat switch`), defined at `src/cli.rs:4917`. Either field optional; at least one required.

**Wire.** It sends the `SetChatExecutor` IPC (`src/commands/service/ipc.rs:602`).

**Daemon handler** — `handle_set_coordinator_executor` (`ipc.rs:1909`), whose doc-comment is the
canonical description of the mechanism:

1. Load/clone `CoordinatorState` for the chat; set `executor_override` and/or `model_override`;
   `state.save_for(dir, id)` — the override is now durable on disk.
2. Read the live handler's pid from the chat dir's session-lock holder; if alive, **SIGTERM it**.
3. The supervisor's `subprocess_coordinator_loop` sees `child.wait()` return, its **restart branch
   fires**, and the next spawn reads `WG_EXECUTOR_TYPE=<new>` + the new model.
4. **History is preserved**: the conversation lives in
   `chat/coordinator-<N>/{inbox,outbox}.jsonl`, shared across handler generations, so the new
   handler "sees prior turns on startup."

**Resume reuses the same IPC.** `wg chat resume` reconstructs `(executor, model)` from
`CoordinatorState.*_override` → falling back to the chat task's `executor_preset_name` / `task.model`
(`reconstruct_resume_metadata`, `chat_cmd.rs:561`) and re-issues `SetChatExecutor`. So "resume"
and "switch model" are the **same respawn path** with different inputs.

**Where the model is bound today.** A chat is a task carrying `executor_preset_name`,
`command_argv`, and `task.model` (`src/chat_command.rs`). The handler subprocess is launched with
that model baked into its argv/env. Even the in-process `nex` handler resolves its model **once**
at construction from config (`nex_runtime` → `resolve_model_for_role(TaskAgent)`), so today *every*
handler binds its model per-process — which is exactly why a switch currently requires a respawn.

**Net:** WG's per-chat switch = persist-override → kill → supervisor-respawn → re-read JSONL. It
reaches the same end state as Pi's `set_model` (new model continues the same conversation) but via
a cold restart rather than a warm in-process mutation.

---

## 5. Recommendation: mid-chat model switching for the WG chat agent

Two layers — ship the verb now (cheap, reuses everything in §4), pursue the warm fast-path later.

### 5.1 Layer 1 — user-facing verb + TUI picker (recommended, low-risk)

- **Add `wg chat model <id> <spec>`** as an ergonomic alias over the existing `SetChatExecutor`
  IPC. It should derive the handler from the spec via `handler_for_model(spec)` (so
  `wg chat model 3 codex:gpt-5.5` switches both model *and* handler, and
  `wg chat model 3 nex:qwen3-coder` flips to the in-process handler) — i.e. the user types one
  model spec, WG sets both `model_override` and the implied `executor_override`. This mirrors Pi's
  `set_model {provider, modelId}` where provider rides with the model.
- **Add a TUI `/model` picker** (Pi's `/model` UX) to the chat pane: list models from the model
  registry / active profile (`wg models`, `src/models.rs:363`), selection issues the same IPC.
  This is the one missing UX affordance — WG's TUI currently exposes a model field only at chat
  *create* time, not mid-chat (`src/tui/viz_viewer/chat_*`).
- **Optionally `wg chat model <id> --cycle`** = Pi's `cycle_model`, rotating over the active
  profile's model list or the fast→standard→premium tiers (`Tier::escalate`, `config.rs:1392`).
- Accept the **respawn cost** for `claude`/`codex`/`pi` handlers — their model is fixed at CLI
  launch, so a respawn is unavoidable and already correct. Surface it honestly in the UI
  ("restarting handler on <model>…"); history is preserved, so it is a brief reconnect, not data loss.

### 5.2 Layer 2 — warm swap on the in-process `nex` path (optional, larger)

To match Pi's zero-respawn swap for the handler WG fully controls (`native`/`nex`):

- Teach `nex_runtime` to **re-resolve its model per turn** from `CoordinatorState.model_override`
  (and the model registry, which `nex` already merges) instead of binding once at construction.
- On `SetChatExecutor` where old and new handler kinds are both `nex`, **skip the SIGTERM**: write
  the override and let the running loop pick it up on the next turn — a true warm swap.
- Keep the cold respawn whenever the handler *kind* changes (e.g. `nex → claude`), since that
  genuinely is a different subprocess.
- This is the natural home for a `set_thinking_level` analogue too (nex already has thinking
  support), giving WG parity with Pi's live reasoning-depth control.

### 5.3 Why this shape

- It is **additive and low-risk**: Layer 1 is a thin verb + picker over an IPC that already works
  and already preserves history — no new state machine.
- It **respects handler reality**: CLI handlers can't swap warm; the in-process handler can, and
  Layer 2 targets exactly that one.
- It **keeps `handler_for_model` as the single source of truth** (`handler_for_model.rs:21`):
  one model spec in, handler + wire derived — the user never has to think about executors.

---

## 6. pi-as-chat / TUI front-end over WG — feasibility

The task explicitly frames a Pi front-end as a place where **terminal takeover is desired, not a
bug** (unlike the OpenCode alt-screen problem, which broke tmux scrollback for *unattended* WG
handlers). Findings:

### 6.1 Two distinct integration shapes (keep them separate)

1. **Pi as a WG worker/chat *handler*** (headless, WG renders). Use Pi `--mode rpc`
   (JSON-over-stdio, no alt-screen) so WG draws the transcript itself in its own ratatui pane —
   the §2.4 event stream maps onto WG's chat events; `set_model` maps onto WG's per-chat switch.
   This is the "strongest embed fit" from `openrouter-cli-executor-scan.md` and the `pi:<model>`
   handler sketch in `evaluate-pi-as-wg-executor.md`. Terminal takeover here **is** a bug (it would
   reintroduce the alt-screen/tmux problem), so this path must use RPC/print, never the TUI.
2. **Pi TUI as an attached human front-end** (Pi renders, full-screen). A user explicitly launches
   Pi's interactive TUI bound to a WG chat session. Here the full-screen takeover is the *product*:
   the human wants Pi's `/model` picker, steering, fork/clone, thinking control, HTML export. This
   is legitimate **iff** it is an explicitly-attached surface, never WG's default unattended
   contract (interactive Pi does not exit on credential failure — it sits in the TUI — so it cannot
   be a worker handler without a supervisor timeout + explicit input protocol; see
   `evaluate-pi-as-wg-executor.md` §"PTY/TUI Launch Shape").

### 6.2 Identity bridge (the key enabler)

Pi's session model maps onto WG chat identity almost 1:1, which makes the front-end coherent:

- WG chat id (`chat-<N>`) ↔ Pi `--session-id <wg-chat-id>` with
  `--session-dir <wg-state-dir>/pi-sessions`.
- WG `chat/<ref>/{inbox,outbox}.jsonl` ↔ Pi session JSONL (both are append-only turn logs).
- WG `wg chat resume` (respawn from saved metadata) ↔ Pi `--continue` / `--resume` /
  `switch_session`.
- WG per-chat `model_override` ↔ Pi `set_model` (the front-end's `/model` selection would write the
  WG override so the choice survives a WG-side respawn).

So a Pi-as-front-end can attach to an existing WG chat, render it full-screen, let the human switch
models via Pi's native `/model`, and have that choice round-trip back into WG's `CoordinatorState`.

### 6.3 Feasibility verdict

- **Feasible, and the cleanest of the two is RPC-handler first** (terminal-safe by construction),
  with the **attached-TUI front-end as an explicit, advanced, user-launched mode** layered on top.
- **Gating unknowns** (carry from `evaluate-pi-as-wg-executor.md`, still open — no credentials on
  the eval machine): credentialed RPC streaming end-to-end, session resume after process kill, and
  large tool-output truncation (`fullOutputPath`). These need a live-provider validation pass before
  WG depends on Pi for either shape.
- **Maintenance caveat:** WG already has `claude`/`codex`/in-process `nex` handlers; Pi overlaps
  them. Adopt Pi where it adds something WG lacks — the **warm `/model` UX** and an **attachable
  human TUI** — rather than as a third redundant worker engine.

---

## 7. Suggested follow-ups (for `pi-design-integration`)

1. **Spec `wg chat model <id> <spec>` + TUI `/model` picker** over the existing `SetChatExecutor`
   IPC (Layer 1, §5.1). Pure verb/UX; no new switch engine.
2. **Design the warm `nex` per-turn re-resolve** (Layer 2, §5.2): per-turn model from
   `CoordinatorState.model_override`; skip SIGTERM when handler kind is unchanged.
3. **Prototype the Pi `--rpc` handler** (`pi:<model>` prefix → `ExecutorKind::Pi`), mapping Pi's
   event stream (§2.4) to WG chat events and `set_model` to WG's per-chat switch. Reuse the
   `evaluate-pi-as-wg-executor.md` launch sketch.
4. **Identity-bridge spike** (§6.2): WG chat id → Pi `--session-id`/`--session-dir`, round-trip the
   model choice into `CoordinatorState`.
5. **Live-credential validation pass** for the three gating unknowns in §6.3 before any first-class
   promotion.

---

## Sources

**WG codebase (verified, this branch):**
- `src/dispatch/handler_for_model.rs:21,71` — single-source-of-truth prefix → `ExecutorKind`; mapping table `:28`.
- `src/config.rs:1362` `Tier`; `:1383` `default_alias` (fast=haiku, standard/premium=opus); `:1392` `escalate`; `:2433` `resolve_tier`; `:2478` `resolve_model_for_role`; `:1413` `ModelRegistryEntry`.
- `src/commands/profile_cmd.rs:720` `use_profile` (profile-as-swap); `:826` `trigger_daemon_reload` → IPC `Reconfigure{profile}`.
- `src/commands/service/ipc.rs:602` `SetChatExecutor` IPC; `:1909` `handle_set_coordinator_executor` (persist override → SIGTERM → supervisor respawn; history via `chat/coordinator-<N>/{inbox,outbox}.jsonl`).
- `src/cli.rs:4917` `wg chat set-executor` (alias `switch`).
- `src/commands/chat_cmd.rs:561` `reconstruct_resume_metadata`; `:590` `run_resume`.
- `src/chat_command.rs` — chat task carries `executor_preset_name` / `command_argv` / `task.model`.
- `src/nex_runtime.rs` — `nex` resolves model once at construction (`resolve_model_for_role(TaskAgent)`).
- `src/profile/templates/{claude,codex,nex,opencode}.toml` — starter profiles (claude=opus, codex=gpt-5.5, nex=`nex:qwen3-coder-30b` @ localhost:8088).

**Pi (external):**
- Repo: https://github.com/earendil-works/pi · site/docs: https://pi.dev/ , https://pi.dev/docs/latest · npm: `@earendil-works/pi-coding-agent`.
- RPC docs: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/rpc.md (`set_model`, `cycle_model`, `get_available_models`, event stream).
- Models docs: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/models.md (`models.json` schema, api shims, `/model` live reload).

**Prior WG reports (reused):**
- `docs/reports/evaluate-pi-as-wg-executor.md` — Pi-as-executor evaluation, smoke tests, `pi:<model>` sketch.
- `docs/research/openrouter-cli-executor-scan.md` — broad CLI-executor scan; Pi = strongest embed fit via `--rpc`.
