# Broad scan: emerging OpenRouter / OpenAI-compatible CLI executors for WG

**Task:** `research-openrouter-cli-executors` · **Date:** 2026-06-14
**Corrective follow-up to** [`opencode-executor-alternatives.md`](opencode-executor-alternatives.md)
(which scoped only OpenCode / Aider / Codex / Claude). This pass casts a wider net,
starts from broad discovery sources, and verifies every recommended candidate against
primary repos/docs.

## TL;DR

- **The single biggest WG-fit predictor is "does it have a non-alt-screen mode?"** —
  i.e. a headless / JSONL / RPC / line-oriented surface WG can drive itself, rather
  than an embedded full-screen TUI on the terminal **alternate screen** (the exact
  thing that breaks tmux scrollback for OpenCode — see prior report). Almost every
  modern coding-agent CLI ships a full-screen TUI **and** some headless surface; the
  headless surface is the embeddable one.
- **The best "embed-by-construction" candidates** (structured stdout WG renders itself,
  no alt-screen): **Pi** (`--rpc` JSON-over-stdio), **Cline CLI** (`--json` NDJSON
  stream), **OpenHands CLI** (`--headless --json` + `--resume`). All three are
  OpenRouter / OpenAI-compatible and actively maintained. These are the **top 3 to
  prototype** (§3).
- **The best line-oriented *interactive* external agent remains Aider** (prior report) —
  `prompt_toolkit` inline, no alt-screen by design. Still the cleanest external choice
  if WG wants a real interactive pane rather than a WG-rendered transcript.
- **Lowest-risk fallback for plain OpenRouter chat** (not a full coding agent):
  **OrChat** — a line-oriented OpenRouter REPL, streaming, pip-installable, no
  file-editing machinery to sandbox (§4).
- **Full-screen-TUI-first tools repeat the OpenCode problem** when embedded interactively:
  **Crush** (Bubble Tea), **Qwen Code** / **Continue** / **Cline** *interactive* mode
  (Ink/React), **VT Code** / **Smelt** (Rust TUIs). Use their headless modes, not their
  TUIs, inside WG.
- **Several named seeds are negative findings:** **Roo Code** (archived May 2026),
  **Kilo Code** (IDE-first; CLI is secondary), **Agent Swarm** / **swarmclaw** (multi-agent
  orchestrators, not a chat executor). Detail in §6.
- **Skepticism flag:** the `bradAGI/awesome-cli-coding-agents` list mixes real repos with
  an implausible "Claw / OpenClaw" ecosystem (entries claiming 100k–378k stars). Treat the
  list as a *discovery* index only; every star count / claim below is anchored to the
  primary repo, and the wildest entries are explicitly called out as unverified (§5, §6).

---

## 1. Candidate matrix (24 tools)

Legend — **Cat**: CA = coding-agent · Chat = chat-only · Harness = orchestrator/multi-agent ·
IDE = IDE-first. **OR** = native OpenRouter. **OAI** = arbitrary OpenAI-compatible
`base_url`. **Term modes**: TUI = full-screen (alt-screen) · REPL = line-oriented inline ·
HL = headless single-shot · JSON/RPC = structured stream WG can render.
Star counts are approximate and **anchored to the primary repo / awesome-list** as of
2026-06; treat as activity signal, not precise.

| # | Tool | Cat | Repo (primary) | License | Activity / stars | Install | OR | OAI | Terminal modes | WG embed risk |
|---|------|-----|----------------|---------|------------------|---------|----|-----|----------------|---------------|
| 1 | **Nex / native** (WG) | CA/Chat | in-tree | (WG) | active | built-in | ✅ | ✅ | WG renders chat | **None** (baseline) |
| 2 | **Aider** | CA | [Aider-AI/aider](https://github.com/Aider-AI/aider) | Apache-2.0 | ~46k, active 2026 | `pip`/`uv` | ✅ `openrouter/…` | ✅ | **REPL (inline, no alt-screen)**, HL (`--message`) | **Low** |
| 3 | **Codex CLI** | CA | [openai/codex](https://github.com/openai/codex) | Apache-2.0 | ~90k, active | npm/brew | via `model_providers` | ✅ | TUI **+ `--no-alt-screen`**, HL (`exec`) | **Low** (already a WG handler) |
| 4 | **Claude CLI** | CA | anthropics/claude-code | source-avail | active | npm | proxy/base-URL only | partial | TUI (no opt-out) | Low integ, poor for OR+tmux |
| 5 | **OpenCode** (status quo) | CA | [anomalyco/opencode](https://github.com/anomalyco/opencode) | OSS | ~172k, active | binary/npm | ✅ | ✅ | **TUI (no opt-out)**, HL (`run --format json`), `serve` API | TUI = the known problem |
| 6 | **Crush** | CA | [charmbracelet/crush](https://github.com/charmbracelet/crush) | FSL/MIT* | ~25k, active | brew/npm/go | ✅ | ✅ (`openai-compat` type) | **TUI (Bubble Tea, alt-screen)**, HL (`crush run`) | TUI = same risk; HL ok |
| 7 | **Goose** | CA/Harness | [block/goose](https://github.com/block/goose) | Apache-2.0 | ~38–48k, active (LF) | binary/desktop | ✅ | ✅ (vLLM/Ollama) | TUI, HL (`goose run`), API | TUI risk; HL/API ok |
| 8 | **Continue CLI (`cn`)** | CA | [continuedev/continue](https://github.com/continuedev/continue) (`extensions/cli`) | Apache-2.0 | ~33k, active | npm/curl | ✅ | ✅ | TUI (Ink), **HL (`-p`)** | TUI risk; `-p` single-shot ok |
| 9 | **Qwen Code** | CA | [QwenLM/qwen-code](https://github.com/QwenLM/qwen-code) | Apache-2.0 | ~25k, active | npm | ✅ | ✅ (`OPENAI_BASE_URL`) | TUI (Ink fork of gemini-cli), **HL (`-p`/`--prompt`)** | TUI risk; HL ok |
| 10 | **Pi** | CA | [badlogic/pi-mono](https://github.com/badlogic/pi-mono) | OSS | ~60k (awesome) | npm (`@mariozechner/pi-coding-agent`) | ✅ | ✅ | TUI, **print (HL)**, **`--rpc` (JSON/stdio)**, SDK | **Low via RPC/print** |
| 11 | **Cline CLI** | CA | [cline/cline](https://github.com/cline/cline) | Apache-2.0 | ~63k, active | npm (Node 22+) | ✅ | ✅ ("any OpenAI-compatible") | TUI, one-shot, **`--json` (NDJSON)**, yolo, zen daemon | **Low via `--json`** |
| 12 | **OpenHands CLI** | CA | [OpenHands/OpenHands-CLI](https://github.com/OpenHands/OpenHands-CLI) | MIT | ~76k (parent), active | `uv tool install openhands` | ✅ (LiteLLM) | ✅ (LiteLLM) | TUI, **`--headless --json`**, **`--resume`** | **Low via headless+resume** |
| 13 | **VT Code** | CA | [vinhnx/vtcode](https://github.com/vinhnx/vtcode) | OSS (crates.io) | emerging 2026 | `cargo`/brew | ✅ | ✅ (21 providers + custom) | TUI (Rust), **ask/exec CLI**, resume | TUI risk; ask/exec ok |
| 14 | **Octomind** | CA/Harness | [Muvon/octomind](https://github.com/Muvon/octomind) | Apache-2.0 | emerging (v0.29, 2026) | `cargo`/binary | ✅ | ✅ | session-first CLI, MCP | Needs verify (TUI vs line) |
| 15 | **Dexto** | CA/Harness | [truffle-ai/dexto](https://github.com/truffle-ai/dexto) | OSS | ~0.6k, active | npm | ✅ | ✅ (50+ LLMs) | CLI (`/commands`, streaming), web, API | Harness-flavored; verify |
| 16 | **Neovate Code** | CA | [neovateai/neovate-code](https://github.com/neovateai/neovate-code) | MIT | ~1.5k (Ant) | npm | likely ✅ | likely ✅ | CLI, plugin system | Emerging; verify modes |
| 17 | **open-codex** | CA | [ymichael/open-codex](https://github.com/ymichael/open-codex) | OSS | ~2.2k | npm | ✅ | ✅ (Chat Completions) | Codex-CLI-style TUI + quiet | Fork of old Codex; verify |
| 18 | **Nanocoder** | CA | [Nano-Collective/nanocoder](https://github.com/Nano-Collective/nanocoder) | OSS | emerging | npm | ✅ | ✅ (BYO model) | CLI | Community; verify modes |
| 19 | **OrChat** | **Chat** | [oop7/OrChat](https://github.com/oop7/OrChat) | OSS | small, active | `pip install orchat` | ✅ (OR-native) | n/a (OR only) | **REPL (line-oriented, streaming)** | **Low — fallback pick** |
| 20 | **Autohand Code CLI** | CA | [autohandai/code-cli](https://github.com/autohandai/code-cli) | OSS | ~136★ | npm | ✅ (default) | ✅ | TUI ("self-evolving") | Small/new; verify |
| 21 | **SoulForge** | CA | [proxysoul/soulforge](https://github.com/proxysoul/soulforge) | OSS | emerging | — | ✅ (21 providers) | ✅ | symbol-edit CLI, multi-agent dispatch | Emerging; verify |
| 22 | **Smelt** | CA | [leonardcser/smelt](https://github.com/leonardcser/smelt) | MIT | ~23★ | `cargo` | ✅ (multi-provider) | ✅ | **TUI (Rust), four modes** | Tiny; TUI risk |
| 23 | **Kilo Code** | **IDE** | [Kilo-Org/kilocode](https://github.com/Kilo-Org/kilocode) | MIT | ~20k, active | VS Code/JetBrains + CLI | ✅ | ✅ | IDE-first; CLI secondary | **Negative** for chat pane |
| 24 | **Roo Code** | **IDE** | [RooCodeInc/Roo-Code](https://github.com/RooCodeInc/Roo-Code) | OSS | ~24k, **archived 5/2026** | VS Code ext | ✅ | ✅ | IDE ext | **Negative** (archived) |

\* Crush license: Charmbracelet uses FSL-1.1 (converts to MIT/Apache after 2y) on recent
projects — confirm exact terms in-repo before redistribution.

---

## 2. WG live-chat executor fit — terminal-mode analysis

WG's failure mode (from the prior report) is embedding a child that owns the **alternate
screen**: tmux copy-mode then finds no scrollback, because the alt-screen only holds the
current frame. The fix space is three-fold, in increasing order of WG control:

1. **Line-oriented / inline REPL** (no alt-screen): output flows into normal scrollback.
   *Aider* (`prompt_toolkit` inline), *OrChat* (Rich-to-stdout REPL). Lowest friction —
   embed as a PTY pane and tmux "just works."
2. **Headless single-shot** (`-p` / `run` / `exec`): no interactive UI at all; WG owns the
   loop and re-invokes per turn. *Continue `cn -p`*, *Crush `run`*, *Qwen `-p`*, *Goose
   `run`*, *Codex `exec`*, *OpenCode `run`*. Great for dispatched/worker chat (this is
   already how `opencode-handler` works), weaker for a *live* interactive feel.
3. **Structured stream (JSON/RPC) that WG renders itself** — the strongest fit, because
   WG draws the conversation in its own ratatui pane (exactly like Nex/native), so there
   is **no embedded child TUI and no alt-screen** by construction:
   - *Pi* `--rpc` — JSON protocol over stdio (purpose-built for embedding/SDK).
   - *Cline CLI* `--json` — NDJSON event stream for piping into other tools.
   - *OpenHands CLI* `--headless --json` + `--resume` — structured + session resume.

### Cross-cutting embeddability criteria

| Criterion | Best in class | Notes |
|---|---|---|
| **tmux/PTY scrollback safe** | Pi (rpc), Cline (json), OpenHands (json), Aider (inline), OrChat | Anything that doesn't take the alt-screen. Avoid Crush/Qwen/VT *TUI* mode. |
| **Model on argv** | Crush `--model {prov}/{model}`, Aider `--model openrouter/…`, Qwen `-m`, Pi, Cline | Most pass `--model`; provider often a prefix or config block. |
| **Endpoint on env** | Qwen (`OPENAI_BASE_URL`), OpenHands/LiteLLM (`*_API_BASE`), Cline, VT Code | OpenRouter is just `base_url=https://openrouter.ai/api/v1` + key for OAI-compat tools. |
| **Session resume** | OpenHands `--resume`, Cline (zen daemon), Crush (multi-session), Aider (`--restore-chat-history`) | Resume maps cleanly onto WG chat continuity. |
| **JSON/RPC stream** | Pi `--rpc`, Cline `--json`, OpenHands `--json`, OpenCode `run --format json` | The structured surface WG should target for live chat. |
| **Impl. complexity (new handler)** | Low: Pi/Cline/OpenHands (structured) · Med: Aider/VT (PTY adapter) · High: TUI-only tools | Structured modes ≈ the existing single-shot handler pattern, extended to stream. |

---

## 3. Top 3 to prototype in WG (new, beyond Aider/Codex)

All three expose a **structured headless/RPC surface** so WG renders chat itself
(no alt-screen, tmux-safe by construction), are OpenRouter / OpenAI-compatible, and are
actively maintained. Smoke-test commands below assume `OPENROUTER_API_KEY` is exported.

### 3.1 Pi — `--rpc` JSON-over-stdio  *(strongest embed fit)*
Purpose-built RPC mode is the closest external analogue to Nex/native: WG speaks JSON,
draws the transcript itself.
```bash
npm i -g @mariozechner/pi-coding-agent
# single-shot (print mode) sanity check:
pi --provider openrouter --model anthropic/claude-3.5-sonnet \
   -p "say hello in one word"
# embedding surface — RPC over stdio (what WG would wire to):
pi --rpc   # then write/read newline-delimited JSON requests on stdin/stdout
```
**Expected:** print mode emits the final answer to stdout and exits 0; `--rpc` opens a
JSON request/response loop on stdio with no alt-screen takeover. Verify the exact RPC
envelope in the repo before wiring (see `@mariozechner/pi-coding-agent` docs/DeepWiki).

### 3.2 Cline CLI — `--json` NDJSON stream
Mature (~63k★), 30+ providers incl. "any OpenAI-compatible endpoint," explicit JSON mode.
```bash
npm i -g cline            # requires Node 22+
cline --json --yolo \
  --provider openrouter --model anthropic/claude-3.5-sonnet \
  "list the files in this directory"
```
**Expected:** a stream of NDJSON events (tool calls, text deltas, completion) on stdout —
WG parses and renders these in its own pane. `--yolo` skips approval prompts for an
unattended smoke run; drop it for interactive. Confirm exact flag spelling against
`cline --help` (one-shot vs `--json` vs `zen` daemon).

### 3.3 OpenHands CLI — `--headless --json` + `--resume`
Strongest *session* story (resume), LiteLLM under the hood → OpenRouter for free.
```bash
uv tool install openhands --python 3.12
export LLM_MODEL="openrouter/anthropic/claude-3.5-sonnet"
export LLM_API_KEY="$OPENROUTER_API_KEY"
openhands --headless --json -t "print the current directory tree, then stop"
# resume the same conversation later:
openhands --resume
```
**Expected:** headless run emits structured JSON events and exits; `--resume` re-attaches
to the prior conversation (maps onto WG chat continuity). LiteLLM means any of its 100+
providers work by swapping `LLM_MODEL`/`LLM_API_BASE` — good hedge against OpenRouter
outages. Note: OpenHands runs a sandboxed runtime; confirm the headless path doesn't
require Docker for the smoke test (the lightweight CLI binary aims to avoid it).

> **Why not Crush/Qwen/Continue as the *interactive* pick:** all three are excellent
> agents but their interactive mode is a full-screen TUI (Bubble Tea / Ink) on the
> alternate screen — i.e. the OpenCode problem again. They're fine as **headless**
> handlers (`crush run`, `qwen -p`, `cn -p`) but don't add a *new* capability over the
> existing single-shot pattern, so they're second-tier for *live chat* specifically.

---

## 4. Low-risk fallback: OrChat (plain OpenRouter chat)

For "I just want OpenRouter chat in a pane," a full coding agent is overkill (file tools,
sandboxing, approval prompts all need handling). **OrChat** ([oop7/OrChat](https://github.com/oop7/OrChat),
[PyPI](https://pypi.org/project/orchat/)) is a **line-oriented OpenRouter REPL** — streaming
responses, markdown rendering, token/cost tracking, multi-line input — with optional,
human-approved shell access (off by default). No alt-screen, no edit/diff machinery.
```bash
pip install orchat
export OPENROUTER_API_KEY=...
orchat            # interactive REPL; /model to switch, streaming output inline
orchat --model anthropic/claude-3.5-sonnet
```
**Expected:** an inline REPL that streams tokens into normal terminal scrollback (tmux-safe),
selectable model via `/model` or `--model`. Cleanest fallback because it is *only* chat —
nothing to sandbox. Runner-up fallback: [mrgoonie/openrouter-cli](https://github.com/mrgoonie/openrouter-cli)
("all-in-one CLI for the OpenRouter API, agent-friendly by default") if a more scriptable,
agent-shaped OpenRouter CLI is wanted.

---

## 5. Mature/stable vs genuinely-emerging (2026)

**Mature / stable** (large stars, sustained 2026 releases, real org backing):
Aider (Apache-2.0), Codex CLI (OpenAI), Claude CLI (Anthropic), OpenCode (Anomaly),
Goose (Block → Linux Foundation), Continue (`cn`), Cline, Qwen Code (Alibaba, Apache-2.0),
Crush (Charmbracelet), OpenHands (All-Hands-AI), Kilo Code. These are safe to depend on;
the only WG question is *which mode* you embed, not whether the tool survives.

**Genuinely emerging 2026** (smaller, newer, less battle-tested — verify before depending):
VT Code (Show HN 2026, Rust), Octomind (Muvon, v0.29 May 2026), Dexto (truffle-ai, ~0.6k),
Neovate Code (Ant Group, ~1.5k), Nanocoder (Nano Collective), open-codex (ymichael fork of
the *old* Codex), Autohand Code CLI (~136★), SoulForge (proxysoul), Smelt (~23★), Pi
(fast-moving monorepo). Pi is the standout *emerging-but-credible* pick for embedding
because of its RPC mode.

**Skepticism / unknowns (called out explicitly):**
- **`bradAGI/awesome-cli-coding-agents` is a discovery index, not ground truth.** It lists
  a parallel "Claw / OpenClaw" ecosystem (Claw Code 193k, Hermes Agent 187k, OpenClaw 378k,
  nanobot 43.9k, etc.) with star counts that are **not plausible** for tools with little
  independent footprint. I could not corroborate these against real GitHub repos and treat
  them as **unverified / likely seeded or satirical**. None are recommended.
- **OpenRouter "top apps" token rankings** (e.g. "Hermes 4.94T, Kilo 1.22T") come from
  third-party blog aggregators (macgpu/vpsmac), not OpenRouter's primary API — directional
  only, not a basis for selection.
- **Marketing superlatives** ("world's fastest," "self-evolving," "smarter than senior devs")
  on Autohand / Qwen reviews / SoulForge are vendor/blog claims, not verified benchmarks.
- **Modes not yet hands-on-verified** (read from docs/READMEs, not run here): Octomind,
  Dexto, Neovate, Nanocoder, SoulForge, Autohand, Smelt terminal-mode details (TUI vs line
  vs headless). A 10-minute `--help` spike per tool would confirm before any of them is
  promoted past the emerging tier.

---

## 6. Negative findings (do not pursue for WG live chat)

- **Roo Code** — **archived May 2026**; superseded by Kilo Code. Do not build on it. ([repo](https://github.com/RooCodeInc/Roo-Code))
- **Kilo Code** — **IDE-first** (VS Code / JetBrains extension that absorbed Roo + Cline UX).
  A standalone CLI exists but is secondary; the product's center of gravity is the editor,
  so it's a poor fit for a terminal chat pane. ([repo](https://github.com/Kilo-Org/kilocode))
- **Agent Swarm** ([docs.agent-swarm.dev](https://docs.agent-swarm.dev)) and **swarmclaw**
  ([swarmclawai/swarmclaw](https://github.com/swarmclawai/swarmclaw)) — **multi-agent
  orchestrators / runtimes** (teams of agents, Slack/GitHub integration, delegation). These
  are harness-class tools that would *contain* an executor, not *be* a WG chat executor.
  Relevant to WG's coordinator layer, not to the live-chat pane.
- **Crush / Qwen Code / Continue / Cline / VT Code / Smelt — *interactive TUI* mode** — all
  use full-screen TUIs on the alternate screen (Bubble Tea for Crush; Ink/React for the
  Gemini-CLI-derived ones; Rust TUIs for VT/Smelt). Embedding their *TUI* reproduces the
  OpenCode tmux-scrollback problem. **Use their headless/JSON modes instead** — that's why
  Crush/Qwen/Continue land in §3's "not the interactive pick" note rather than the top 3.
- **Claude CLI for OpenRouter** — OpenRouter only reachable via base-URL skin or a proxy
  ([y-router](https://github.com/luohy15/y-router)); also alt-screen with no opt-out. Fine
  for Anthropic, wrong tool for OpenRouter chat (carried from prior report).
- **OpenCode interactive** — alt-screen, no opt-out; issues #106 / #5809 closed unfixed
  (carried from prior report). Keep the existing scroll-key workaround; don't expect upstream.

---

## 7. Recommendation

1. **Prototype Pi (`--rpc`) first.** It's the external tool closest to Nex/native: WG owns
   the render path via a JSON-over-stdio loop, so tmux scrollback is correct by construction,
   and OpenRouter is first-class. Lowest terminal-integration risk of any *external* agent.
2. **Prototype Cline `--json` and OpenHands `--headless --json --resume` in parallel** as the
   mature, well-staffed alternatives — Cline for breadth of OpenAI-compatible providers,
   OpenHands for session resume + LiteLLM provider hedging.
3. **Keep Aider** (prior report) as the line-oriented *interactive* option when WG wants a
   real PTY pane rather than a WG-rendered transcript — it's the only mature agent that is
   inline (no alt-screen) by default.
4. **Ship OrChat as the low-risk OpenRouter *chat* fallback** for users who don't need a
   coding agent — nothing to sandbox, line-oriented, pip-installable.
5. **Do not embed any tool's full-screen TUI** for live chat. For Crush/Qwen/Continue, wire
   their headless single-shot modes into the existing handler pattern if they're wanted as
   *worker* executors, but they add nothing over the current single-shot path for *chat*.

### Suggested follow-up tasks (not yet created)
- Spike: WG handler for **Pi `--rpc`** (define the JSON envelope mapping to WG chat events).
- Spike: WG handler for **Cline `--json`** / **OpenHands `--headless --json`**, compare
  stream schemas and pick one streaming contract for WG to standardize on.
- 10-min `--help` verification pass on the emerging tier (Octomind, Dexto, Neovate,
  Nanocoder, SoulForge, Autohand, Smelt) to confirm terminal modes before promoting any.
- Confirm Crush's exact license (FSL vs MIT/Apache) before any redistribution.

---

## Sources (primary, verified)

**Discovery seeds**
- awesome-openrouter (apps index): https://github.com/OpenRouterTeam/awesome-openrouter · apps.json: https://github.com/OpenRouterTeam/awesome-openrouter/blob/main/apps.json
- Works with OpenRouter: https://openrouter.ai/works-with-openrouter · CLI agent category: https://openrouter.ai/apps/category/coding/cli-agent
- bradAGI/awesome-cli-coding-agents (use skeptically — see §5): https://github.com/bradAGI/awesome-cli-coding-agents
- OpenRouter "build a headless agent" / "agent TUI" cookbooks: https://openrouter.ai/docs/guides/coding-agents/create-headless-agent · https://openrouter.ai/docs/cookbook/building-agents/create-agent-harness-tui

**Recommended candidates**
- Pi — https://github.com/badlogic/pi-mono · npm: https://www.npmjs.com/package/@mariozechner/pi-coding-agent · modes (interactive/print/RPC/SDK): https://deepwiki.com/badlogic/pi-mono/4-@mariozechnerpi-coding-agent
- Cline — https://github.com/cline/cline · CLI: https://cline.bot/cli · OpenRouter: https://docs.cline.bot/provider-config/openrouter
- OpenHands CLI — https://github.com/OpenHands/OpenHands-CLI · parent: https://github.com/OpenHands/OpenHands · SDK paper: https://arxiv.org/html/2511.03690v1
- Aider OpenRouter — https://aider.chat/docs/llms/openrouter.html · releases: https://github.com/Aider-AI/aider/releases
- OrChat — https://github.com/oop7/OrChat · https://pypi.org/project/orchat/
- mrgoonie/openrouter-cli — https://github.com/mrgoonie/openrouter-cli

**Other matrix entries**
- Crush — https://github.com/charmbracelet/crush · non-interactive `run` (v0.34.0): https://github.com/charmbracelet/crush/releases/tag/v0.34.0 · CLI-only request #1030: https://github.com/charmbracelet/crush/issues/1030
- Goose — https://github.com/block/goose · providers: https://github.com/aaif-goose/goose/blob/main/documentation/docs/getting-started/providers.md
- Continue CLI — https://github.com/continuedev/continue/tree/main/extensions/cli · docs: https://docs.continue.dev/guides/cli
- Qwen Code — https://github.com/QwenLM/qwen-code
- VT Code — https://github.com/vinhnx/vtcode · OpenRouter doc: https://github.com/vinhnx/vtcode/blob/main/docs/providers/openrouter.md · Show HN: https://news.ycombinator.com/item?id=48332098
- Octomind — https://github.com/Muvon/octomind · release notes: https://muvon.io/blog/release-round-may-2026
- Dexto — https://github.com/truffle-ai/dexto · docs: https://docs.dexto.ai/docs/getting-started/intro
- Neovate Code — https://github.com/neovateai/neovate-code
- open-codex — https://github.com/ymichael/open-codex
- Nanocoder — https://github.com/Nano-Collective/nanocoder · nanocode (NanoGPT): https://github.com/nanogpt-community/nanocode
- Autohand Code CLI — https://github.com/autohandai/code-cli · OpenRouter: https://openrouter.ai/works-with-openrouter/autohand
- SoulForge — https://github.com/proxysoul/soulforge · https://soulforge.proxysoul.com/introduction
- Smelt — https://github.com/leonardcser/smelt
- Kilo Code — https://github.com/Kilo-Org/kilocode
- Roo Code (archived) — https://github.com/RooCodeInc/Roo-Code
- Agent Swarm — https://docs.agent-swarm.dev/docs/getting-started · swarmclaw — https://github.com/swarmclawai/swarmclaw

**Carried from prior report** (`opencode-executor-alternatives.md`)
- OpenCode alt-screen issues #106 / #5809; Codex `--no-alt-screen` PR #8555; prompt_toolkit inline semantics; y-router. See that doc's §4 for full links.
