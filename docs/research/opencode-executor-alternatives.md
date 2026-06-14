# OpenCode terminal behavior & executor alternatives for WG live chat

**Task:** `research-opencode-executors` · **Date:** 2026-06-14

## TL;DR

- **The OpenCode alt-screen / tmux-scrollback problem is real, public, and unfixed
  upstream.** OpenCode's interactive TUI runs on the terminal **alternate screen**,
  which bypasses the host terminal's (and tmux's) scrollback. The two relevant
  upstream issues — a direct "let me disable the alternate screen" request
  ([#106](https://github.com/anomalyco/opencode/issues/106), opened 2025-06-14)
  and "support scrolling in tmux copy mode"
  ([#5809](https://github.com/anomalyco/opencode/issues/5809), opened 2025-12-19) —
  are **both closed with no inline / no-alt-screen mode shipped**.
- **There is NO officially supported OpenCode mode that behaves well inside
  WG/tmux while staying interactive.** OpenCode's non-TUI modes (`opencode run`,
  `opencode serve`, `opencode web`) are *non-interactive* / API / browser surfaces,
  not a line-oriented interactive terminal chat. (See [CLI docs](https://opencode.ai/docs/cli/).)
  WG's current scroll-key-forwarding workaround in `src/tui/pty_pane.rs` is therefore
  the only viable path *if we keep OpenCode for live chat*.
- **Better-behaved, well-maintained, OpenRouter-capable alternatives exist:**
  - **Nex / native WG** — no embedded-child-TUI problem *by construction* (WG renders
    chat itself). OpenRouter via the `openrouter:` / `nex:` model prefix. Lowest risk.
  - **Aider** — uses `prompt_toolkit` **inline** (no alternate screen), so output flows
    into normal terminal/tmux scrollback naturally. Native OpenRouter support. Actively
    maintained. Best-behaved *external* CLI for line-oriented interactive chat.
  - **Codex CLI** — shipped an official **`--no-alt-screen`** flag + `tui.alternate_screen`
    config in Jan 2026 ([PR #8555](https://github.com/openai/codex/pull/8555), merged
    2026-01-09) — the exact knob OpenCode lacks. OpenRouter via custom `model_providers`.
    Already wired into WG as the `codex` handler.
- **Recommendation (see §5):** keep OpenCode + the scroll-key workaround for users who
  specifically want it, but steer the **default OpenRouter live-chat** path toward
  **Nex/native** (zero TUI risk) and, if a polished external agent feel is wanted,
  add an **Aider** or lean on the existing **Codex `--no-alt-screen`** adapter.
  Confidence: high on the OpenCode negative finding; medium on the Aider/Codex
  ergonomics for *chat-only* use (unknowns called out inline).

---

## 1. The OpenCode alternate-screen / tmux-scrollback issue

### What WG sees today

WG embeds the full-screen OpenCode TUI in a PTY pane for live chat
(`src/chat_command.rs:132` — `"opencode"` launches `opencode` with `--model`; the
comment notes "OpenCode launches its own TUI for an interactive PTY chat"). Because
OpenCode owns its own alternate screen, tmux copy-mode finds no scrollback history to
walk (the alt-screen only ever holds the current repaint frame). WG works around this
in `src/tui/pty_pane.rs` (`child_scroll_keys`, `fix-opencode-tui`): for OpenCode panes,
WG's scroll controls forward OpenCode's *own* scroll keys (PageUp/PageDown/Home/End)
into the PTY instead of driving tmux copy-mode. See `src/tui/viz_viewer/state.rs:14918`.

### Is it a known/public issue? Yes.

| Issue | Repo | Opened | Status | Notes |
|---|---|---|---|---|
| [#106 "Option to not use alternate (fullscreen) screen mode"](https://github.com/anomalyco/opencode/issues/106) | anomalyco/opencode | 2025-06-14 | **Closed** | Direct ask: output to the normal buffer so users get terminal/multiplexer scroll/search/copy instead of OpenCode's reimplementation. Cites #100, #102 (custom-scroll & keybinding conflicts). No inline mode shipped. |
| [#5809 "[FEATURE]: Support scrolling in tmux copy mode"](https://github.com/anomalyco/opencode/issues/5809) | anomalyco/opencode | 2025-12-19 | **Closed** | "Cannot scroll backwards through conversation history in tmux copy-mode — the session renders as a single output block." Labeled `discussion` / `opentui` (the v1.0 TUI is built on [OpenTUI](https://github.com/anomalyco/opentui), a Zig+TS terminal-UI core). No fix recorded. |

Repo note: OpenCode moved from `sst/opencode` to **`anomalyco/opencode`** when SST
rebranded to Anomaly in 2026. It is **actively maintained** (releases in the v1.15.x
line during 2026), so the absence of an inline mode is a *product choice*, not abandonment.

### Is there an official no-alt-screen / headless interactive mode? No.

Per the [OpenCode CLI docs](https://opencode.ai/docs/cli/), the non-TUI surfaces are:

- `opencode run "<prompt>"` — **non-interactive** single-shot (this is what WG's
  `opencode-handler` already uses for worker/dispatched chat: `opencode run --format json`).
- `opencode serve` — **headless HTTP API** server (no terminal chat UI).
- `opencode web` — headless server + **browser** web UI.

None of these is a *line-oriented interactive terminal chat*. The interactive `opencode`
/ `opencode tui` path has **no documented flag or config to disable the alternate
screen** or render inline. Negative finding confirmed against the CLI docs and the two
closed issues above.

> **Contrast — what "good" looks like upstream:** OpenAI's Codex shipped exactly this
> knob. [PR #8555](https://github.com/openai/codex/pull/8555) (merged **2026-01-09**,
> released in v0.81.0-alpha.1) added a `--no-alt-screen` CLI flag and a
> `tui.alternate_screen = auto|always|never` config; `auto` even auto-detects Zellij and
> drops the alt-screen so scrollback keeps working
> ([related Codex issue #2836](https://github.com/openai/codex/issues/2836)). Claude Code
> has an *open, unshipped* request for the same
> ([anthropics/claude-code #38283](https://github.com/anthropics/claude-code/issues/38283)).
> OpenCode is the laggard here.

---

## 2. Candidate comparison for WG live chat

WG's handler routing (`src/dispatch/handler_for_model.rs`) already maps model-spec
prefixes to handlers: `claude:*`→claude CLI, `codex:*`→codex CLI, `nex:*`/`openrouter:*`/
`ollama:*`/`vllm:*`→native (in-process Nex, OpenAI-compatible), and external agents
(`opencode:*`, `aider:*`, `goose:*`) are addressed by executor prefix.

| Candidate | Maintained? | OpenRouter / OAI-compat | Alt-screen in tmux | Line-oriented / inline interactive? | Model/provider ergonomics | WG integration risk |
|---|---|---|---|---|---|---|
| **Nex / native WG** | Yes (WG's own code) | Native: `openrouter:*`, `nex:*`, any OAI-compatible `base_url` | **N/A — WG renders chat itself** (no embedded child TUI; no alt-screen) | Yes — WG owns the render path, tmux scrollback works | Model spec prefix; `wg config -m openrouter:<vendor>/<model>` | **Lowest** — already in-process |
| **Aider** | Yes — releases through 2026 (commit 2026-03, repo updated 2026-05) | **Native** `--model openrouter/<vendor>/<model>` ([docs](https://aider.chat/docs/llms/openrouter.html)) | **No alt-screen** — `prompt_toolkit` runs *non-full-screen* (inline), output flows to normal scrollback | **Yes — best-in-class for this**; `/ask` & `--chat-mode ask` for chat-only | `--model openrouter/...`; rich model aliases | **Medium** — new handler/PTY adapter; Python (pip/uv) dep; git/edit-oriented |
| **Codex CLI** | Yes (OpenAI) | Via `model_providers` in `~/.codex/config.toml` (`base_url=https://openrouter.ai/api/v1`, `wire_api="chat"`) | **Optional** — `--no-alt-screen` flag + `tui.alternate_screen=never` (since v0.81.0-alpha.1) | Yes with `--no-alt-screen` (inline) | config.toml provider blocks; `--model` | **Low** — already a WG handler (`codex`) |
| **Claude CLI** | Yes (Anthropic) | Indirect: `ANTHROPIC_BASE_URL=https://openrouter.ai/api` ("Anthropic skin") or a proxy ([y-router](https://github.com/luohy15/y-router)) | **Alt-screen**; has its own open tmux scroll/mouse issues ([#38810](https://github.com/anthropics/claude-code/issues/38810), [#38283](https://github.com/anthropics/claude-code/issues/38283)) | No official inline flag (unshipped) | Anthropic-first; OpenRouter is a bolt-on | **Low** integration (already a handler), but **poor** for OpenRouter + tmux |
| **OpenCode** (status quo) | Yes (Anomaly) | Native OpenRouter (`opencode:openrouter/<vendor>/<model>`) | **Alt-screen, no opt-out** (issues #106/#5809 closed unfixed) | No | `--model provider/model`; OpenRouter-first | **Already integrated**, but needs WG's scroll-key workaround to be usable |

### Notes per candidate

- **Nex / native WG.** The only candidate with *no* embedded-child-TUI failure mode:
  WG draws the conversation in its own ratatui pane, so tmux scrollback and copy-mode
  behave normally. OpenRouter and any OpenAI-compatible endpoint are first-class
  (`handler_for_model.rs:32-39`). The trade-off is that Nex is WG's own agent loop, not a
  third-party coding agent with its own tool ecosystem — fine for "OpenRouter-style chat,"
  less so if users want OpenCode/Codex agentic file-editing.
- **Aider.** The standout *external* tool for this problem: `prompt_toolkit` in
  non-full-screen mode means **no alternate screen**, so it composes cleanly with tmux
  scrollback/copy-mode — structurally the opposite of OpenCode. Native OpenRouter
  (`aider --model openrouter/<vendor>/<model>`), actively maintained, and has an explicit
  chat-only mode (`/ask`, `--chat-mode ask`). Unknowns: it's primarily a *code-editing*
  agent (git-aware, writes diffs); embedding it purely as live chat needs a small adapter
  and a Python runtime dependency.
- **Codex CLI.** Already integrated in WG. Its **`--no-alt-screen`** flag is the exact
  remedy OpenCode lacks, and OpenRouter works via a `model_providers` block
  (`wire_api="chat"`, `base_url=https://openrouter.ai/api/v1`). If we want an external
  agentic TUI that *also* behaves in tmux, Codex-with-`--no-alt-screen` is the
  lowest-effort win.
- **Claude CLI.** Best for Anthropic models, but OpenRouter is only reachable via a
  base-URL skin or a translation proxy, and it shares the same alt-screen/tmux scroll
  pain (with no shipped opt-out). Not the right tool for OpenRouter-style chat.

---

## 3. Confidence & unknowns

- **High confidence:** OpenCode has no official no-alt-screen/inline interactive mode
  (CLI docs + #106 + #5809, all consistent); Codex shipped `--no-alt-screen` (merged PR);
  Aider is inline/non-full-screen by design (prompt_toolkit semantics); Nex/native has no
  embedded-TUI issue (WG source).
- **Medium confidence / unknowns:**
  - Whether OpenCode exposes an *undocumented* env var to suppress the alt-screen — not
    found in docs or issues, but source wasn't read line-by-line. Worth a 10-min source
    grep before committing to the workaround long-term.
  - Aider's exact ergonomics as a *chat-only* (non-editing) live executor inside a WG PTY
    pane, and whether to wrap it as a PTY pane vs. a single-shot handler like
    `opencode-handler`. Needs a spike.
  - Codex `--no-alt-screen` rendering quality inside WG's specific PTY pane (vs. Zellij,
    which it was built for) — untested here.

---

## 4. Sources & commands checked

**Codebase (grounding):** `src/chat_command.rs:132` (opencode TUI launch), `src/tui/pty_pane.rs`
(`child_scroll_keys` workaround, ~lines 119, 626, 752-790, 2220), `src/tui/viz_viewer/state.rs:14918`,
`src/commands/opencode_handler.rs` (`opencode run --format json` single-shot), `src/dispatch/handler_for_model.rs`
(prefix→handler routing table).

**Primary upstream sources:**
- OpenCode #106 — no-alt-screen request (CLOSED): https://github.com/anomalyco/opencode/issues/106
- OpenCode #5809 — tmux copy-mode scrolling (CLOSED): https://github.com/anomalyco/opencode/issues/5809
- OpenCode CLI docs (run/serve/web; no inline flag): https://opencode.ai/docs/cli/
- OpenTUI (OpenCode's v1 TUI core): https://github.com/anomalyco/opentui
- Codex `--no-alt-screen` PR #8555 (MERGED 2026-01-09): https://github.com/openai/codex/pull/8555
- Codex alt-screen issue #2836: https://github.com/openai/codex/issues/2836
- Codex OpenRouter config: https://openrouter.ai/docs/cookbook/coding-agents/codex-cli
- Aider OpenRouter docs: https://aider.chat/docs/llms/openrouter.html
- Aider releases (maintenance): https://github.com/Aider-AI/aider/releases
- prompt_toolkit full-screen-vs-inline semantics: https://github.com/prompt-toolkit/python-prompt-toolkit/blob/3.0.33/docs/pages/full_screen_apps.rst
- Claude Code tmux scroll/mouse issues #38283, #38810: https://github.com/anthropics/claude-code/issues/38283 , https://github.com/anthropics/claude-code/issues/38810
- Claude Code + OpenRouter (base URL / proxy): https://openrouter.ai/docs/guides/coding-agents/claude-code-integration , https://github.com/luohy15/y-router

**Negative findings (explicit):**
- No OpenCode flag/config/env documented to disable the alternate screen or run the
  interactive TUI inline (checked CLI docs + #106 + #5809).
- OpenCode's `run`/`serve`/`web` are non-interactive/API/browser — none is a line-oriented
  interactive terminal chat.
- Claude CLI has no shipped inline/no-alt-screen mode (request open, unshipped).

---

## 5. Recommendation (practical path)

1. **Keep OpenCode + the scroll-key-forwarding workaround as-is** for users who want
   OpenCode specifically. There is no upstream inline mode to adopt and both relevant
   issues are closed without one, so the workaround is the realistic ceiling for OpenCode.
   Do **not** invest in chasing an OpenCode no-alt-screen mode — it doesn't exist.
2. **Make Nex/native the default OpenRouter live-chat path.** It sidesteps the entire
   embedded-TUI problem (WG renders chat itself; tmux scrollback just works), is already
   in-process, and supports OpenRouter via the `openrouter:` prefix. Lowest risk, highest
   terminal-integration quality. *(If Nex's chat UX needs polish, that's the cheapest
   investment of the options here.)*
3. **For a polished external-agent feel that still behaves in tmux, prefer Codex with
   `--no-alt-screen`** over OpenCode — it's already a WG handler and gains correct tmux
   scrollback with a one-flag change; OpenRouter via a `model_providers` block.
4. **Optionally add an Aider live-chat adapter** as the best-behaved *external* line-oriented
   chat (inline `prompt_toolkit`, native OpenRouter, no alt-screen). Worth a spike if WG wants
   a third-party agent whose terminal behavior is correct by construction rather than via a
   keystroke-forwarding workaround.

**Suggested follow-up tasks** (not yet created — see §3 unknowns):
   grep OpenCode source for any hidden alt-screen env var; spike a Codex `--no-alt-screen`
   live-chat default; spike an Aider chat-only adapter and compare tmux scrollback behavior
   against the OpenCode workaround.
