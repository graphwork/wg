# Octomind & Dexto live-chat executor prototype — verification + design

**Task:** `prototype-octomind-dexto-chat` · **Date:** 2026-06-14
**Input:** [`openrouter-cli-executor-scan.md`](openrouter-cli-executor-scan.md) §5 flagged both
Octomind and Dexto as *emerging, terminal-mode unverified*. This note records the hands-on
`--help` / PTY verification the scan deferred, and the resulting WG integration design.

## TL;DR

Both tools are **viable** and BOTH are **better than OpenCode** for tmux scrollback
(neither takes the terminal alternate screen). Verified locally:

| Tool | Version | Install | Live-chat command | Model spelling | Alt-screen? |
|------|---------|---------|-------------------|----------------|-------------|
| **Octomind** | 0.31.0 | `cargo install octomind --locked` | `octomind run -m openrouter:<vendor>/<model> -n <session>` | **native WG spelling** `openrouter:minimax/minimax-m3` | **No** (line-oriented REPL) |
| **Dexto** | 1.8.11 | `npm i -g dexto` | `dexto --agent <per-chat.yml> --auto-approve` | via generated agent YAML (`llm.provider: openrouter`, `model: minimax/minimax-m3`) | **No** (Ink, inline) |

## Octomind — verified

- `octomind --help` exposes: `run` (interactive REPL **or** `--format plain|jsonl` headless),
  `server` (WebSocket), `acp` (ACP over stdio), `send`, `workflow`, `--daemon`.
- `octomind run -m openrouter:minimax/minimax-m3` (interactive) prints a banner
  `Role: assistant:concierge · Model: openrouter:minimax/minimax-m3` — **the typed model is
  preserved verbatim and displayed**, no silent fallback. Starts a named, resumable session
  (`-n <name>` resumes if it exists; also `--resume`, `--resume-recent`).
- `echo "hi" | octomind run -m openrouter:minimax/minimax-m3 --format jsonl` reaches OpenRouter
  and fails **only** with `401 Unauthorized: Missing Authentication header` on a dummy key —
  proving the route is preserved end-to-end (no fallback). OpenRouter auth is taken from
  `OPENROUTER_API_KEY` in the environment; **no config file is required** (built-in defaults).
- PTY capture of interactive `run`: **no** `ESC[?1049h` / `1047h` / `47h` (alt-screen) — it is a
  line-oriented REPL, so tmux copy-mode scrollback works normally. **Strictly better than the
  OpenCode alt-screen TUI.** WG therefore keeps the normal tmux copy-mode path for octomind
  panes (does NOT call `set_child_scroll_keys`).
- **Chosen WG live-chat mode:** interactive `octomind run -m <model> -n <session> --sandbox`.
  `-m` takes WG's exact `openrouter:<vendor>/<model>` spec, so model preservation is trivial.

## Dexto — verified

- `dexto --help`: default `--mode cli` (Ink REPL), `run` headless, `--mode web|server|mcp`,
  `-m/--model`, `-a/--agent <id|path>`, `--auto-approve`, `-c/--continue`, `-r/--resume`.
- **`-m` does NOT accept OpenRouter routes.** `dexto run -m minimax/minimax-m3` →
  `Model 'minimax/minimax-m3' looks like an OpenRouter-format ID (provider/model). Please set
  provider/model explicitly in agent config for this command.` A bare known id like
  `gpt-4o-mini` infers `provider=openai`. The global chat action has **no `--provider` flag**
  (`--provider` exists only on the `setup`/`connect` subcommands), so the only reliable way to
  use OpenRouter with an arbitrary typed model is an **agent config YAML**.
- Verified working route: a minimal YAML
  ```yaml
  systemPrompt: "..."
  llm:
    provider: openrouter
    model: minimax/minimax-m3
    apiKey: $OPENROUTER_API_KEY
  ```
  launched via `dexto --agent <path> run "..."` echoes `model: minimax/minimax-m3 ·
  provider: openrouter` and fails **only** at the dummy-key `401` — model preserved, no
  fallback. WG generates this YAML per-chat (`<chat_dir>/dexto-agent.yml`) at launch.
- PTY capture of interactive Ink CLI: **no** alt-screen sequence (`\e[2J` clear only). tmux
  scrollback works — **better than OpenCode**. WG keeps the normal copy-mode path for dexto.
- **Chosen WG live-chat mode:** `dexto --agent <generated-yml> --auto-approve` (interactive
  Ink CLI). `--auto-approve` keeps tool prompts from blocking the unattended pane.

## WG integration design (this task)

1. `ExecutorKind::Octomind` / `::Dexto` added to `EXTERNAL_CLIS` (so `octomind:`/`dexto:` are
   recognized executor prefixes) but **NOT** to `WORKER_ONLY_EXTERNALS` — like OpenCode they
   are chat-capable, so the live-chat guard admits them.
2. TUI `[+]` new-chat: `ADD_NEW_EXECUTOR_CHOICES` gains `octomind` and `dexto`. Both are
   OpenRouter-first (model route selects the provider), so — like OpenCode — neither shows the
   Endpoint field; the Model fuzzy-dropdown is offered.
3. Model preservation:
   - octomind → `chat_command::octomind_model_arg()` normalizes any typed spelling
     (`octomind:openrouter:minimax/minimax-m3`, `openrouter:minimax/minimax-m3`, bare
     `minimax/minimax-m3`) to octomind's `openrouter:<vendor>/<model>` `-m` value.
   - dexto → `chat_command::dexto_openrouter_model()` extracts the raw `<vendor>/<model>`
     route and writes it into the generated agent YAML's `llm.model`.
4. Launch argv is built in `tui::viz_viewer::state` (TUI PTY path) and `chat_command`
   (`argv_for_preset`, the stored command record). Octomind is pure; dexto writes its per-chat
   YAML via `chat_command::write_dexto_agent_config(chat_dir, model)`.
5. **Scope note (prototype):** the daemon-managed `spawn-task` HandlerSpec path is NOT wired
   for octomind/dexto — they currently launch only through the TUI live-chat PTY path (which
   owns `chat_dir` and PTY sizing). `spawn-task` returns a clear "TUI-chat-only" error for
   them, and the lightweight agency one-shot path degrades them to the safe claude-haiku
   default, exactly like the other non-chat externals. A follow-up task can add full handlers.
