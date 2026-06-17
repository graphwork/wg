# nex tool-framework compatibility audit & minimal-tools default decision

**Task:** `audit-nex-tool` — *Audit nex tool-framework compatibility; decide minimal-tools default.*
**Date:** 2026-06-17
**Scope:** Research/design only. No production code changes. This is the task artifact.
**Builds on:** `make-nex-tool` (commit `fed87953`), which added the runtime
context-length probe this decision hinges on.

---

## TL;DR

1. **Minimal-tools default → CONDITIONAL, probe-driven.** Do *not* make
   `--minimal-tools` a blanket default (it hobbles capable models) and do *not*
   leave it purely opt-in (small/local models, the ones that actually need it,
   never get it). Auto-enable the lean surface when the `make-nex-tool` context
   probe resolves an effective context window **≤ 32k tokens**, keep the full
   surface above that, and keep an explicit `--minimal-tools` / new
   `--full-tools` override pair that always wins. Surface the auto-decision with
   a one-line banner so it is discoverable the moment it bites.
2. **Tool-calling today is solid for two wire formats** — native Anthropic
   Messages `tool_use` and OpenAI Chat Completions `function`/`tool_calls`, with
   a clean translation layer and a text-tag fallback parser for small models.
   **No OpenAI Responses API. No native Gemini.** MCP is real but v1
   (stdio-only, `tools/list`/`tools/call`, opt-in).
3. **The canonical tool names do NOT mirror Claude Code.** They are snake_case
   (`read_file`, `edit_file`, `write_file`, `bash`, `grep`, `glob`, …), and
   `todo_write`/`TodoWrite` — named in the CLI help and in the minimal-tools
   allowlist — **is not a registered tool anywhere.** Two real, cheap bugs.
4. **External-harness integration is scaffolded but unvalidated.** Command
   templates + discovery for opencode/aider/goose/qwen/cline (stable) and
   crush/amplifier/octomind/dexto (experimental) exist as *real code*, but the
   most recent arena smoke ran with **none of them installed** — every external
   row is BLOCKED. The scaffolding is real; the live integration is aspirational.

Prioritized roadmap is in §5.

---

## 1. Should `--minimal-tools` be the default? — CONDITIONAL (recommended)

### 1.1 What the flag does today

`wg nex --minimal-tools` (`src/nex_cli.rs:159-165`) narrows the tool registry to
a fixed allowlist via `ToolRegistry::keep_only_tools` and also implies `--no-mcp`
(`src/commands/nex.rs:160-161`, `234-246`):

```
read_file, edit_file, write_file, bash, grep, glob, todo_write
```

It also swaps the system-prompt tool summary for a minimal variant that tells the
model `web_search`/`web_fetch` are gone and to use `bash` + `curl`/`wget`
instead (`src/commands/nex.rs:657-664`, tested at `nex.rs:804-815`).

The full default surface (`ToolRegistry::default_all_with_config_and_routing`,
`src/executor/native/tools/mod.rs:435-511`) is ~17 tools: the 6 local-dev tools
above **plus** `web_search`, `arxiv_search`, `web_fetch`, `bg`, `delegate`,
`summarize`, `research`, `deep_research`, `reader`, `map`, `chunk_map` — each
carrying a JSON-schema definition that is re-sent in the prefill of **every**
turn.

### 1.2 The tradeoff

- **Cost of the full surface:** every tool's schema is prefill, every turn. For a
  small local model on an 8k–32k context window, ~17 verbose schemas can eat a
  meaningful fraction of the window and slow prefill (which on a local llama.cpp
  box is already the bottleneck). The lean set is ~6 tools.
- **Cost of the minimal surface:** capable models lose `web_search`/`web_fetch`
  (must fall back to `bash curl`), `delegate`/`summarize`/`research`/`reader`/
  `map`/`chunk_map` (the whole sub-agent + large-context toolbox), and all MCP
  tools. On a 200k-context frontier model this is pure capability loss for no
  meaningful prefill saving.

So the right answer is **neither blanket nor purely opt-in** — it is a function
of the model/endpoint, which is exactly what `make-nex-tool` made measurable.

### 1.3 Tie-in to the `make-nex-tool` context probe

`make-nex-tool` added `src/executor/native/context_probe.rs` +
`resolve_context_window(...)` and wired it into `provider.rs:559-572`. By the
time the registry is built, nex can resolve the **effective** context window with
real precedence: *explicit config > live probe (`/props` `n_ctx`, `/v1/models`
`max_model_len`) > model registry > configurable fallback*. For a llama.cpp
server booted `-c 8192` this now returns `8192`, not a blind `128000`.

That number is the natural conditional signal, because the thing minimal-tools
optimizes (prefill pressure) is precisely "how much of a small window do the tool
schemas eat." The same probe value already drives the tool-output channeling
threshold (`channel.rs:threshold_for_context_window`); reusing it for the tool
**surface** keeps a single source of truth.

### 1.4 Recommendation

**CONDITIONAL default, probe-driven:**

1. After provider creation, read the resolved context window
   (`client.context_window()`).
2. If **no explicit `--minimal-tools`/`--full-tools` flag** was passed and the
   resolved window is **≤ `minimal_tools_context_threshold`** (new
   `[native_executor]` key, default **32_000**), build the lean surface;
   otherwise build the full surface.
3. Both explicit flags always win over the auto-decision.
4. Emit one banner line on stderr (non-eval) when the auto-path fires, e.g.
   `wg nex — minimal tool surface (8k context detected); /tools full to expand`.

Why 32k: it is the inflection where the full-surface schemas stop being a
material fraction of the window, it matches the channeling clamp's lower band,
and it cleanly separates "small/local" (8k–32k llama.cpp/vLLM) from "capable"
(Qwen3-Coder-30B at 256k, OpenRouter frontier at 128k–1M). Make it a config knob
so operators can tune per fleet. Context window is a better probe-backed proxy
than the `local` provider hint (some local servers run huge windows) — but if
desired, the hint can be a secondary nudge (`local` + unknown window → lean).

Caveat to fix alongside (see §3 of the small-model triage,
`docs/triage-wg-nex-small-model-reports-2026-04-27.md`): the minimal prompt should
also name the `bash python3` escape hatch and `web_fetch`-saves-binaries
behavior, so an auto-minimized small model is not left guessing.

### 1.5 Addendum — runtime toggle + the "hidden default" question

The task addendum asks for a runtime toggle (`/tools` or `/minimal` slash command
and/or `--minimal-tools`/`--full-tools` flag pair) and whether minimal should be
the **hidden-but-default** state (clap `hide`), favoring a clean default UI.

**Recommendation: visible conditional default + discoverable toggle; do NOT make
minimal a silent blanket hidden default.**

- A capability-reducing default that is *hidden* is a least-surprise violation:
  the user cannot tell why the model "can't" fetch a URL, and nothing on screen
  points them at the fix. The arena/triage history already shows small models
  mis-reporting stripped tools as platform bugs. The conditional default avoids
  this by (a) only firing for models where it helps and (b) printing the banner
  at the moment it bites.
- **REPL toggle:** add `/tools` to the existing slash-command surface
  (`agent.rs:handle_nex_slash_command`, advertised in `/help` at
  `agent.rs:3839`):
  - `/tools` — show current surface (mode + count + names),
  - `/tools minimal` / `/tools full` — switch live.
- **Flag pair:** keep `--minimal-tools`, add `--full-tools` (the explicit
  "override the conditional default upward"). It is acceptable to clap-`hide`
  *both* flags from the top-level help **only because** the banner + `/help`
  `/tools` entry carry discoverability — the capability is never invisible at the
  point of use. Do not hide it without that anchor.

**Implementation note (important):** `keep_only_tools` is *destructive* — it drops
the `Box<dyn Tool>` instances, so you cannot toggle back to full without
rebuilding the whole registry (and re-running MCP discovery). For a live `/tools`
toggle, build the **full** registry once and gate it with a non-destructive
*active allowlist* view instead: an `Option<HashSet<String>>` consulted by
`definitions()` (the surface sent to the model) and `execute()`. `None` = full,
`Some(set)` = lean. This keeps both states free to switch between and lets
`/tools` report state accurately. The current `keep_only_tools` path can remain
for the one-shot CLI flag, but the toggle needs the view model.

---

## 2. What tool-calling / framework compatibility exists TODAY

### 2.1 Architecture

Canonical types are **Anthropic-shaped** (`Message`, `ContentBlock::{ToolUse,
ToolResult}`, `ToolDefinition`) in `src/executor/native/client.rs`. Every backend
implements the `Provider` trait (`provider.rs:24-44`, including
`fn context_window()`), so the agent loop never sees wire format. Routing lives in
`provider.rs:create_provider_ext...` and `src/dispatch/handler_for_model.rs`.

### 2.2 Wire formats

| Wire format | Status | Where | Notes |
|---|---|---|---|
| **Anthropic Messages `tool_use`** | ✅ native, no translation | `client.rs` (`AnthropicClient`, `Provider` impl ~L514) | Streaming SSE parser, retry/backoff, cache-token accounting. Used for `claude`-family / `anthropic` routes. |
| **OpenAI Chat Completions `function`/`tool_calls`** | ✅ full translate roundtrip | `openai_client.rs` (`OpenAiClient`) | `translate_tools` → `{type:"function",...}` (L444), `tool_choice:"auto"` set when tools present (L68, L790) — many OpenRouter models silently ignore tools without it. `/chat/completions` (L815). SSE streaming with partial-tool-call accumulation. Covers OpenRouter, OpenAI, Ollama, vLLM, llama.cpp, Together, DeepSeek, etc. |
| **Text-tag tool-call fallback** | ✅ for non-compliant models | `openai_client.rs:extract_tool_calls_from_text` (L1944+) | Parses tool calls a model emitted as text instead of structured `tool_calls`: Hermes/ChatML `<tool_call>…</tool_call>`, Llama/Fireworks `<function=name>…</function>`, and provider tags `<\|tool_call\|>` / `<minimax:tool_call>`. Overrides `finish_reason` to `tool_calls` so the loop processes them (L735). This is the key small/local-model compatibility lever. |
| **OpenAI Responses API** (`/responses`, `wire_api="responses"`) | ❌ not implemented | — | nex always uses `/chat/completions`. Note: the **codex CLI** handler uses Responses (`docs/reports/executor-arena-smoke.md:24,69`), but that is a separate subprocess handler, not nex. |
| **Native Gemini `generateContent`** | ❌ not implemented | — | `gemini:*` routes go through the OAI-compat client / OpenRouter, not Google's native function-calling endpoint (`handler_for_model.rs:38` notes "per impl"). |

Provider selection (`provider.rs:397-427`): model prefix > explicit override >
endpoint provider > heuristic (`anthropic/` or claude-name → anthropic; contains
`/` → oai-compat) > `WG_LLM_PROVIDER` > **oai-compat fallback** (WG is
local/open-model-first). Credentials are **config-only by contract** — no implicit
`ANTHROPIC_API_KEY`/`OPENAI_API_KEY` env fallback (`provider.rs:450-455,
504-520`). (NB: the 2026-03-23 `native-executor-dual-api-audit.md` predates this
and still describes env fallback + a missing `tool_choice`; both are superseded by
current code — treat that doc as historical for those two points.)

### 2.3 MCP support

Real, but **v1** (`src/executor/native/mcp/`, `mod.rs:12-20`):

- **Transport:** stdio only (SSE/WebSocket/streamable-HTTP deferred,
  `transport.rs:3`).
- **Protocol:** `initialize` + `notifications/initialized` handshake,
  `tools/list`, `tools/call`. Resources and prompts are a follow-up.
- **Lifecycle:** supervised child processes with bounded crash-restart
  (`supervisor.rs`), killed on manager drop.
- **Surfacing:** discovered tools are namespaced `<server>__<tool>` and injected
  into `ToolRegistry` so they are indistinguishable from native tools to the
  agent (`mod.rs:7-10`).
- **Default-on, conditionally:** MCP servers are spawned **iff** `[mcp.servers]`
  is non-empty AND `--no-mcp` was not passed (`commands/nex.rs:258-299`). There
  are **no built-in default servers**, so a stock install surfaces zero MCP tools
  until the user configures one. `--no-mcp` (and `--minimal-tools`, `--eval-mode`)
  force it off.

### 2.4 Canonical tool-set naming — **does NOT mirror Claude Code (finding)**

The task description, the `--minimal-tools` help text (`nex_cli.rs:160`), and
several docs claim the canonical set is *"Read/Edit/Write/Bash/Grep/Glob/
TodoWrite (mirrors Claude Code)."* The actual registered names
(`tools/file.rs:89-95`, `tools/*.rs` `fn name`) are **snake_case**:

```
read_file, write_file, edit_file, glob, grep, bash,
web_search, arxiv_search, web_fetch, bg, delegate, summarize,
research, deep_research, reader, map, chunk_map  (+ note tools)
```

Two concrete consequences:

- **They are not Claude Code's names.** Claude Code uses PascalCase
  `Read`/`Edit`/`Write`/`Bash`/`Grep`/`Glob`/`TodoWrite`. A prompt or harness
  written against Claude Code's tool names will not match nex's. Prompt-pattern
  portability is therefore *partial*.
- **`todo_write` / `TodoWrite` is a phantom.** It is listed in **both**
  `keep_only_tools` allowlists (`commands/nex.rs:244` and the test at `:843`) and
  in the help text, but **no `todo_write` Tool is registered anywhere** (grep
  across `src/`). `keep_only_tools` silently filters to a name that never exists.
  This was already flagged as follow-up #3 in
  `docs/triage-wg-nex-small-model-reports-2026-04-27.md:257-277` and is still open.

---

## 3. External-harness integration — real vs aspirational

There are **two distinct axes** here; the task's "exec-* work" is axis A.

### Axis A — WG drives external CLI agents as workers

`src/executor_discovery.rs` enumerates them and probes PATH:

- **Stable:** `opencode`, `aider`, `goose`, `qwen`(/`qwen-code`), `cline`
  (`STABLE_EXTERNAL_EXECUTORS`, L15).
- **Experimental:** `octomind`, `dexto`, `crush`, `amplifier`
  (`EXPERIMENTAL_EXTERNAL_EXECUTORS`, L17).
- **Core / provider:** `native`(nex), `claude`, `codex`, `shell`, `gemini`.

Command construction and model-arg normalization are **real code** in
`src/commands/spawn/execution.rs`:
- per-executor command templates (`build_inner_command`, L1358; e.g. `opencode`
  L1581, `aider` L1588, `goose` L1595, `qwen` L1602, `cline` L1609, `crush`
  L1616, `amplifier` L1623),
- model-arg *style* per executor (`ProviderSlashModel` / `ProviderFlagAndModel`
  / `BareOpenRouterModel`, L976-979) and append logic that respects user
  `[executor].args` (L1074-1145),
- OpenCode route normalization in `src/chat_command.rs:23-54`.

**Verdict: scaffolding is REAL; live integration is ASPIRATIONAL/unvalidated.**
The most recent arena smoke
(`docs/reports/executor-arena-smoke.md`, 2026-06-01) ran with the model
`deepseek/deepseek-v4-flash` and found:
- `opencode`, `aider`, `goose`, `qwen`, `cline`, `crush`, `amplifier`, `gemini` →
  **all BLOCKED — binary not installed** (rows L26-33). None were exercised
  end-to-end.
- `codex` CLI → **PASS** over OpenRouter using `wire_api="responses"` (L24).
- `claude` CLI → BLOCKED (no cheap OpenRouter route) (L25).
- `wg nex` / standalone `nex` → **FAIL** with OpenRouter 401 (L21-23) — but this
  is an **`api_key_env` credential-propagation bug** on the OpenRouter path
  (env-only keys not reaching the native request path; existing tests cover
  `api_key_file` only), *not* a tool-calling defect. Follow-up noted at L83.

So: the adapters are coded and discoverable, the model-arg shapes are designed,
but **no stable external harness has a passing live smoke** in the record — they
were never installed in the smoke environment. Treat "WG can drive goose/aider/
cline/opencode/qwen today" as *designed and plausibly working, but unproven*.
Crush/amplifier are explicitly experimental (flags "verify against your installed
version", `executor_discovery.rs:137-143`).

Related design/research docs (aspirational, not shipped behavior): the amplifier
series (`docs/research/amplifier-executor-gap.md`,
`amplifier-integration-proposal.md.typ`, `amplifier-context-transfer.md`,
`docs/reports/amplifier-research-report.md`),
`docs/research/opencode-executor-alternatives.md`,
`docs/research/thin-wrapper-executors-2026-04.md`,
`docs/research/openrouter-cli-executor-scan.md`, and the executor-arena
research/ranking set (`docs/reports/executor-arena-{research,ranking,final-integration}.md`).

### Axis B — nex itself as a target inside external eval harnesses

This is what `--eval-mode` is for (`nex_cli.rs:134-149`): non-interactive,
`--autonomous` + `--no-mcp`, no chat-surface pollution, single-line JSON summary
on stdout for SWE-bench / Terminal-Bench-style graders. The Terminal-Bench
campaign is documented under `docs/terminal-bench/` and
`docs/reports/nex-terminal-bench-*.md` (per-model runs:
`v4flash`, `minimax-m27`, `smoke`). These exercise nex's *own* tool loop as a
benchmark target and consistently pair `--eval-mode --minimal-tools` — direct
evidence that the lean surface is the right default for the small-model eval
regime, and corroboration for the §1 conditional recommendation.

---

## 4. (covered in §5)

## 5. Prioritized compatibility roadmap

Ordered by value/cost. Items 1–2 are cheap correctness wins; 3–6 are capability.

**P0 — Tool-naming + dead-entry correctness (cheap, unblocks prompt-compat).**
- Remove `todo_write` from both `keep_only_tools` allowlists *or* implement a
  real `todo_write`/TodoWrite tool (the latter also closes a genuine Claude-Code
  parity gap — agents trained on Claude Code expect a todo tool). Pick implement
  if cheap; otherwise delete. (`commands/nex.rs:244`; triage follow-up #3.)
- Reconcile the "mirrors Claude Code" claim: either (a) fix the help/docs to say
  snake_case, or (b) add Claude-Code PascalCase **aliases** (`Read`→`read_file`,
  etc.) so prompts/harnesses written against Claude Code's tool names work
  unchanged. (b) is the higher-value option for harness portability and is
  low-risk (alias table in the registry).

**P1 — Land the conditional minimal-tools default + `/tools` toggle (this doc).**
- Wire the probe-driven conditional (§1.4), the `--full-tools` flag, the banner,
  and the non-destructive active-allowlist view + `/tools` slash command (§1.5).
- Fold in the small-model prompt hardening from the triage doc (bash/python
  escape hatches, binary-fetch note) so auto-minimized models aren't blind.

**P2 — MCP hardening.**
- Add streamable-HTTP / SSE transport (stdio-only is the biggest ecosystem gap;
  many hosted MCP servers are HTTP).
- Add `resources` and `prompts` (currently deferred, `mcp/mod.rs:14-15`).
- Tighten discovery/restart error surfacing. This is the most-requested,
  highest-leverage capability expansion because MCP multiplies tool reach without
  per-integration Rust code.

**P3 — Anthropic tool-use parity / cost.**
- Explicit `cache_control` blocks on the Anthropic native path (OAI/OpenRouter
  path already sends `cache_control: ephemeral`); system prompt is otherwise
  re-sent uncached every turn.
- Extended-thinking / beta headers for thinking-capable Claude models.
  (Both flagged in `native-executor-dual-api-audit.md` §gaps and still open.)

**P4 — OpenAI Responses API (`wire_api="responses"`).**
- Needed for GPT-5.x-class models and tools that assume Responses semantics
  (reasoning items, server-side state). Codex-over-OpenRouter already proves the
  wire works in the arena. Medium priority — Chat Completions still covers the
  bulk of OAI-compatible endpoints, so this is "reach the newest frontier
  models," not "fix something broken."

**P5 — Native Gemini function-calling + validate Axis-A harnesses.**
- Native `generateContent` for `gemini:*` (today it only works via OpenRouter
  oai-compat). Lower priority while OpenRouter covers Gemini.
- Stand up a real installed-binary smoke for at least the **stable** external
  executors (opencode/aider/goose/qwen/cline) to move them from "scaffolded" to
  "validated," and fix the `api_key_env` OpenRouter credential gap that made nex
  itself FAIL the last arena smoke (`executor-arena-smoke.md:83`).

---

## Appendix — primary source pointers

| Concern | File:line |
|---|---|
| `--minimal-tools` flag + help | `src/nex_cli.rs:159-165` |
| minimal-tools wiring, implies `--no-mcp`, allowlist | `src/commands/nex.rs:160-161, 234-246` |
| minimal vs full system-prompt summary | `src/commands/nex.rs:657-664` (tests `:804-815`) |
| `keep_only_tools` (destructive) | `src/executor/native/tools/mod.rs:222-226` |
| full default surface (~17 tools) | `src/executor/native/tools/mod.rs:435-511` |
| actual tool names (snake_case) | `src/executor/native/tools/file.rs:89-95`, `tools/*.rs fn name` |
| context probe (make-nex-tool) | `src/executor/native/context_probe.rs` (`resolve_context_window` L88) |
| probe wired into provider | `src/executor/native/provider.rs:559-572` |
| channeling threshold from ctx window | `src/executor/native/channel.rs:60, 92` |
| Anthropic native client (`tool_use`) | `src/executor/native/client.rs` (`AnthropicClient`) |
| OpenAI Chat Completions + translation | `src/executor/native/openai_client.rs:444, 790, 815` |
| text-tag tool-call fallback parser | `src/executor/native/openai_client.rs:1944+` |
| provider routing / config-only creds | `src/executor/native/provider.rs:397-427, 450-520` |
| handler-for-model table | `src/dispatch/handler_for_model.rs:30-39` |
| MCP v1 scope (stdio, tools/list+call) | `src/executor/native/mcp/mod.rs:12-20`, `transport.rs:3` |
| MCP spawn (opt-in, `--no-mcp`) | `src/commands/nex.rs:258-299` |
| REPL slash commands (`/help`) | `src/executor/native/agent.rs:3695-3864` |
| registry held by agent loop | `src/executor/native/agent.rs:99` (`tools: ToolRegistry`), `definitions()` re-sent each turn `:1460, 2064, 2190` |
| external executor discovery | `src/executor_discovery.rs:14-145` |
| external executor command build | `src/commands/spawn/execution.rs:976-979, 1358-1650` |
| arena smoke (real-vs-aspirational) | `docs/reports/executor-arena-smoke.md` |
| small-model triage (prompt + todo_write) | `docs/triage-wg-nex-small-model-reports-2026-04-27.md` |
| dual-API audit (historical, partly stale) | `docs/research/native-executor-dual-api-audit.md` |
| context-aware channeling design | `docs/nex-context-aware-channeling.md` |
