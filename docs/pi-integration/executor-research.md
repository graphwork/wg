# Pi.dev as a WG Executor — Integration Research + Terminal-Takeover Root Cause

Task: `pi-research-executor`
Date: 2026-06-22
Status: **Investigation only — no production code changed.**

This document answers two questions:

1. **What blocks** running pi.dev (the *Pi Coding Agent*) as a WG executor/handler?
2. **Why does pi take over the terminal** when WG spawns it "headlessly", and what
   is the *specific* mechanism?

It supersedes the higher-level survey in
[`docs/reports/evaluate-pi-as-wg-executor.md`](../reports/evaluate-pi-as-wg-executor.md)
(task `evaluate-pi-as`, 2026-06-15) by pinning the takeover to exact source
lines and proposing a concrete handler interface + patch-vs-wrapper decision.

---

## 0. What pi is (recap, with what was verified here)

- Project: **Pi Coding Agent** (not Inflection's consumer "Pi").
- npm package: `@earendil-works/pi-coding-agent`, CLI bin `pi` → `dist/cli.js`.
- Version examined here: **0.79.4** (TUI sub-package `@earendil-works/pi-tui` **0.79.9**).
- License: MIT. Repo: https://github.com/earendil-works/pi · Docs: https://pi.dev/docs/latest
- Runtime: **Node.js** (ESM). `dist/cli.js` is `#!/usr/bin/env node`. Not a static binary.
- Launch flow: `dist/cli.js` → `main(process.argv.slice(2))` in `dist/main.js`.

Source was obtained with `npm pack @earendil-works/pi-coding-agent@0.79.4 --ignore-scripts`
(and the same for `@earendil-works/pi-tui`) and read locally; no live LLM turn was run
(no provider credentials on this box — matching the prior report).

pi has **four** runtime modes, selected by `resolveAppMode()` in `dist/main.js`:

| Mode | Trigger | Terminal behavior |
|------|---------|-------------------|
| `interactive` | TTY on **both** stdin & stdout, and no `-p`/`--mode` | **Full-screen TUI — grabs the terminal** |
| `print`  | `-p`/`--print`, **or** stdin not a TTY, **or** stdout not a TTY | one-shot, plain text to stdout, exits |
| `json`   | `--mode json` | one-shot, JSONL event stream to stdout, exits |
| `rpc`    | `--mode rpc` | long-lived, JSONL command/event protocol over stdin/stdout |

`print` / `json` / `rpc` are all headless and **never** enter the TUI. This single
fact drives the whole recommendation below.

---

## 1. Terminal-takeover root cause (the specific mechanism)

### 1.1 The decision point

The takeover is gated entirely by one function, `dist/main.js:77-88`:

```js
function resolveAppMode(parsed, stdinIsTTY, stdoutIsTTY) {
    if (parsed.mode === "rpc")  return "rpc";
    if (parsed.mode === "json") return "json";
    if (parsed.print || !stdinIsTTY || !stdoutIsTTY) return "print";
    return "interactive";
}
```

called as `dist/main.js:399`:

```js
let appMode = resolveAppMode(parsed, process.stdin.isTTY, process.stdout.isTTY);
```

So pi enters the interactive TUI **iff**:

> `parsed.mode === undefined` **and** `parsed.print === false`
> **and** `process.stdin.isTTY === true` **and** `process.stdout.isTTY === true`.

If **either** stdin or stdout is not a TTY, or `-p`/`--mode` is passed, pi is headless.
This is the crux: **the takeover is not unconditional; it requires a TTY on both
fds with no mode flag.**

### 1.2 What "takeover" actually does (the operations)

When `appMode === "interactive"`, `dist/main.js:629` constructs `InteractiveMode`
and `dist/modes/interactive/interactive-mode.js:468` calls `this.ui.start()`, which
reaches the pi-tui terminal driver `@earendil-works/pi-tui` `dist/terminal.js:74-101`:

```js
start(onInput, onResize) {
    this.wasRaw = process.stdin.isRaw || false;
    if (process.stdin.setRawMode) {
        process.stdin.setRawMode(true);          // (1) RAW MODE — the real grab
    }
    process.stdin.setEncoding("utf8");
    process.stdin.resume();                        // (2) consume all keystrokes
    process.stdout.write("\x1b[?2004h");           // (3) bracketed paste on
    process.stdout.on("resize", this.resizeHandler);
    if (process.platform !== "win32") {
        process.kill(process.pid, "SIGWINCH");     // (4) self-kick a resize
    }
    this.enableWindowsVTInput();
    this.queryAndEnableKittyProtocol();            // (5) write Kitty/DA query to
                                                   //     stdout, read reply on stdin
}
```

So the *specific* takeover is:

1. **`process.stdin.setRawMode(true)`** (`terminal.js:80`). This is the core of the
   problem. Raw mode disables the line discipline: no canonical/line buffering, no
   echo, and **no signal generation (ISIG off)** — so Ctrl-C/Ctrl-Z no longer turn
   into SIGINT/SIGTSTP at the tty layer; the terminal is now pi's to drive.
2. **`stdin.resume()` + a persistent `data` listener** (`terminal.js:83`, `:150`):
   pi swallows every byte of input as TUI key events.
3. **Bracketed paste mode** `\x1b[?2004h` (`terminal.js:85`).
4. **A self-directed `SIGWINCH`** to refresh dimensions (`terminal.js:91`).
5. **Kitty-keyboard-protocol / modifyOtherKeys negotiation** (`terminal.js:148-153`):
   pi writes a query sequence to stdout and waits for the reply on stdin. **This is
   the exact source of the `tmux extended-keys is off…` warning** the prior report saw.
6. Full-screen rendering: hide cursor `\x1b[?25l` (`terminal.js:390`), clear-screen
   `\x1b[2J\x1b[H` (`terminal.js:~404`), cursor moves, line clears.
7. **Signal handlers** for terminal restore (`interactive-mode.js:2820-2887`):
   SIGTERM/SIGHUP teardown, SIGTSTP/SIGCONT suspend-resume, and pi **ignores SIGINT
   while suspended** so Ctrl-C in the host shell won't kill it. On exit it restores
   raw state (`terminal.js:358-360`) and re-shows the cursor.

### 1.3 What it deliberately does NOT do

**pi-tui does not use the alternate screen buffer.** There is no `\x1b[?1049h`
(smcup) anywhere in pi-tui's `dist/` — verified by grep. pi renders **inline**, doing
a full-screen repaint with `clearScreen` (`\x1b[2J\x1b[H`) + relative cursor moves.

This matters for diagnosis: the symptom ("my terminal is wrecked / scrollback gone /
keys don't echo / Ctrl-C dead") is **raw mode + the inline full-screen repaint
claiming stdin/stdout**, *not* an alt-screen swap. A naive "send rmcup to fix it"
recovery will not help; the fix is to not enter interactive mode (or to restore raw
mode), not to toggle the alt buffer.

### 1.4 Why WG saw the takeover "when invoking pi non-interactively"

WG's only PTY-spawning path is the TUI chat embed `src/tui/pty_pane.rs`, which uses
`portable-pty` to give a child process a **pseudo-terminal on both ends** (it exists
to render an interactive child's UI inside a ratatui pane, e.g. `wg nex`, octomind,
dexto). If pi is launched down that path — or simply inherits the operator's real
terminal — then `process.stdin.isTTY && process.stdout.isTTY` is **true**, and with
no `--mode`/`-p` flag `resolveAppMode` returns `interactive`. pi then runs
`setRawMode(true)` on that PTY and takes over.

Conversely, WG's **worker** spawn path (`src/commands/spawn/execution.rs`) wires
`Stdio::null()`/piped/file-redirected stdio (e.g. `execution.rs:653-655`,
and the JSONL/stdout split at `:1756-1765`). Down that path neither fd is a TTY, so
pi would auto-select `print` and **not** take over. The takeover is therefore a
property of *which launch path was used*, not of pi being "interactive by nature".

> **Root cause, one line:** pi was given a PTY on both stdin **and** stdout with no
> `--mode`/`-p` flag, so `resolveAppMode` chose `interactive`, and pi-tui's
> `terminal.start()` put that PTY into raw mode and claimed it for a full-screen TUI.

---

## 2. BLOCKERS

Each blocker lists a **root cause** and a **fix path** (patch-pi vs WG-side wrapper).
"Wrapper" = something WG does on its side; "patch" = a change upstreamed into pi.

### B1 — TUI raw-mode terminal grab on both-TTY launch  *(the headline issue)*
- **Root cause:** `resolveAppMode` (`main.js:77-88`) → `interactive` when stdin &
  stdout are both TTYs and no `-p`/`--mode`; `pi-tui terminal.start()` then calls
  `setRawMode(true)` (`terminal.js:80`) and claims the terminal (§1).
- **Fix path — WRAPPER (no patch needed):** launch the worker/chat handler with
  **`--mode rpc`** (live chat) or **`-p` / `--mode json`** (one-shot), and/or spawn
  with piped/null stdio so at least one fd is not a TTY. Either condition alone
  defeats the takeover. This is fully in WG's control.
- **Optional patch (belt-and-suspenders, see §4.2):** a default-off env switch
  `PI_NO_TUI` that forces non-interactive even under a TTY.

### B2 — Interactive mode never exits on error (blocks the worker forever)
- **Root cause:** in interactive mode a credential/error is *rendered into the TUI*
  and pi keeps waiting for input (prior report: PTY launch → `timeout` killed it,
  exit 124). There is no nonzero exit to signal failure.
- **Fix path — WRAPPER:** never run interactive for workers (B1 fix). In `print`
  mode pi exits **nonzero** with an actionable message (prior report: exit 1,
  `No API key found for openrouter.`). Add a supervisor timeout regardless.

### B3 — No codex-style server-side resumable session id
- **Root cause:** pi's session model is **file-based JSONL**, not a daemon with a
  resumable handle. (codex hands back a session id; pi persists to a `.jsonl`.)
- **Fix path — WRAPPER:** pi already exposes everything needed —
  `--session-id <id>`, `--session-dir <dir>` (or `PI_CODING_AGENT_SESSION_DIR`),
  `--continue`, `--resume`, `--no-session`. Map the WG chat/task id →
  `--session-id`, point `--session-dir` at a WG state dir. For multi-turn chat,
  keep one long-lived `--mode rpc` process and stream `prompt` commands (preferred);
  for stateless one-shot workers, the opencode-style "replay transcript every turn"
  pattern (`src/commands/opencode_handler.rs`) also works.

### B4 — Model/provider impedance mismatch with WG's single model-spec string
- **Root cause:** pi takes `--provider <name>` + `--model <pattern>` (supports
  `provider/id` and `:<thinking>`); custom endpoints need a `~/.pi/agent/models.json`
  with `baseUrl`/`api`/`apiKey`/`headers`. WG carries one spec string (`pi:...`).
- **Fix path — WRAPPER:** a `pi:` handler that normalizes the WG spec to
  `--provider`/`--model` (mirror `opencode_model_arg` in
  `src/commands/opencode_handler.rs:273-299` and `chat_command::opencode_model_arg`),
  writing a temp `models.json` only for custom `baseUrl`/headers/key refs that can't
  ride CLI flags. OpenRouter-style ids already work (prior report verified
  `openrouter anthropic/claude-3.5-haiku`).

### B5 — Secrets leak via `--api-key` argv
- **Root cause:** pi accepts `--api-key <key>`, which exposes the secret in the
  process arg list (`/proc/<pid>/cmdline`, `ps`).
- **Fix path — WRAPPER:** pass credentials by **environment** instead
  (`OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`),
  resolved through `wg secret`. Never put the key on argv.

### B6 — Distribution / discovery: pi is an npm package, not a binary
- **Root cause:** running pi requires Node + npm and a `pi` on `PATH` (or `npx`).
  There is no single static artifact.
- **Fix path — WRAPPER:** treat exactly like the other external CLIs — locate `pi`
  via `src/executor_discovery.rs`, and have `wg config lint` reject a `pi:` route
  when no `pi` binary/model is installable (the prior report listed this as a
  pre-first-class smoke test).

### B7 — (NON-blocker / positive) stdout is already protected in headless modes
- pi calls `takeOverStdout()` in every non-interactive mode (`main.js:400-403`,
  `dist/core/output-guard.js`) so stray `console.log`/library writes from tools do
  **not** corrupt the structured stream — the agent's real output goes through
  `writeRawStdout`. This is *better* than scraping a terminal and aligns with WG's
  "stdout-is-protocol" contract (`src/commands/opencode_handler.rs:20-25`). No action
  needed; noted so it isn't re-flagged as a risk.

### B8 — (Needs live validation) credentialed RPC streaming + large tool output
- **Root cause:** unverified on this box (no credentials). RPC `bash` truncation
  with `fullOutputPath`, `agent_end`/`turn_end` timing, and crash-resume were not
  exercised against a real provider.
- **Fix path — WRAPPER + smoke test:** add a credentialed smoke scenario before
  first-class status (see §5).

---

## 3. Executor-handler interface sketch for `pi:`

### 3.1 Where it plugs into WG

- Add `ExecutorKind::Pi` (`as_str() == "pi"`) in `src/dispatch/plan.rs`.
- pi is chat-capable (RPC), so it is an **external CLI that is also chat-capable** —
  like `OpenCode`. Put `Pi` in `ExecutorKind::EXTERNAL_CLIS` but **NOT** in
  `WORKER_ONLY_EXTERNALS` (`src/dispatch/plan.rs:91-118`).
- `handler_for_model` (`src/dispatch/handler_for_model.rs`) already has a generic
  interception for `is_external_cli()` prefixes, so once `Pi` is in `EXTERNAL_CLIS`,
  a route like `pi:openrouter/anthropic/claude-3.5-haiku` routes to
  `ExecutorKind::Pi` automatically — no new match arm (mirror the existing
  `test_opencode_prefix_routes_to_opencode_handler`).
- `pi` is an **executor** name, not a provider prefix — keep it out of
  `KNOWN_PROVIDERS` (same treatment as `opencode`/`aider`).

### 3.2 Shape A — live chat handler (preferred): `wg pi-handler --chat`

Peer of `src/commands/opencode_handler.rs` / `codex_handler.rs`. One long-lived pi
process per chat, driven over the RPC protocol (`docs/rpc.md`).

**Spawn (argv):**
```
pi --mode rpc \
   --provider <prov> --model <model> \
   --session-id <wg-chat-or-task-id> \
   --session-dir <wg-state>/pi-sessions \
   --append-system-prompt <wg-system-prompt-file-or-text> \
   --no-approve
```
**stdio:** `stdin = piped`, `stdout = piped`, `stderr = handler.log`. (Pipes ⇒ not a
TTY ⇒ no takeover even without `--mode rpc`; `--mode rpc` makes it explicit/robust.)

**env:** `PI_CODING_AGENT_DIR`, `PI_CODING_AGENT_SESSION_DIR`, and the provider key
(`OPENROUTER_API_KEY` / `ANTHROPIC_API_KEY` / …) — never `--api-key`.

**Per WG inbox message** (poll loop identical to opencode_handler):
1. Write one JSONL command to pi stdin (LF-delimited; never `\r\n`-only):
   ```json
   {"id":"req-<n>","type":"prompt","message":"<user text>"}
   ```
2. Read pi stdout JSONL events:
   - `{"type":"response","id":"req-<n>","success":true}` ⇒ accepted.
   - `message_update` with `assistantMessageEvent.type == "text_delta"` ⇒ append
     delta to WG `streaming_path` (`chat::streaming_path_ref`) for live display.
   - `{"type":"agent_end", ...}` ⇒ **turn complete** (idle).
3. Capture the final reply: either read it from the `agent_end` messages, or send
   `{"type":"get_last_assistant_text"}` and read its `response.data.text`
   (`docs/rpc.md` §`get_last_assistant_text`). Write to WG outbox
   (`chat::append_outbox_ref`); clear the streaming path.
4. Cancel = `{"type":"abort"}`. Shutdown = SIGTERM (pi exits 143; in print/rpc it
   registers SIGTERM/SIGHUP teardown — `dist/modes/print-mode.js:~26-44`).

**Framing caveat (from `docs/rpc.md`):** split records on `\n` only; a generic
line-reader that also breaks on `U+2028`/`U+2029` (e.g. Node `readline`) is
non-compliant because those bytes occur inside JSON strings. A Rust
`BufRead::read_until(b'\n')` is correct.

### 3.3 Shape B — one-shot worker (print/json): for `WORKER_ONLY` style dispatch

Mirrors the external-CLI worker spawn in `src/commands/spawn/execution.rs`.

**Text:**
```
pi -p \
   --provider <prov> --model <model> \
   --session-id <task-id> --session-dir <wg-state>/pi-sessions \
   --append-system-prompt <wg-system-prompt> --no-approve \
   "<assembled prompt>"        # or feed the prompt on piped stdin
```
**JSON event stream (richer capture):** same with `--mode json` instead of `-p`.

**stdio / capture:** stdin piped (prompt or empty), `stdout` = the result — plain
text (`-p`) or JSONL events (`--mode json`) — captured to `raw_stream.jsonl` /
`output.log` exactly like the claude/codex stdout split (`execution.rs:1756-1765`).
`stderr` = diagnostics. **Exit code:** `0` ok, nonzero on error (e.g. `1` =
`No API key found for <prov>`), which WG treats as task failure. Turn output = the
final assistant text (text mode) or the last `message_end`/`turn_end` text
(JSON mode).

### 3.4 Reusable WG plumbing this mirrors
- Reply extraction → `extract_export_reply` / `extract_reply`
  (`src/commands/opencode_handler.rs:358-447`).
- Model normalization → `opencode_model_arg`
  (`src/commands/opencode_handler.rs:273-299`).
- Session lock + inbox cursor loop → `opencode_handler::run` (`:40-166`).
- Stdout-is-protocol discipline (diagnostics to stderr/`handler.log` only) →
  `src/commands/opencode_handler.rs:20-25`.

---

## 4. Recommendation: WRAPPER (no upstream patch required)

### 4.1 Decision

**Integrate pi via a WG-side wrapper/handler; do not patch pi to fix the takeover.**

Rationale:
- The takeover is **already avoidable from WG's side** (§1.1): launch with
  `--mode rpc` or `-p`/`--mode json`, and/or pipe stdio. pi's headless modes are
  first-class, documented, and stable. There is no missing capability to add.
- pi's non-interactive surface is *good*: clean stdout (`takeOverStdout`, B7), a
  documented RPC protocol with explicit turn/idle events and
  `get_last_assistant_text`, predictable nonzero exits, and a JSONL session store
  that maps onto WG chat identity. None of this needs upstream changes.
- A patch creates a maintenance/version dependency on upstream pi for a problem WG
  can already solve with two flags. Keep the dependency surface zero.

This matches the prior report's conclusion (keep pi as an experimental/custom
executor recipe first; promote to first-class only after credentialed smoke tests),
and sharpens it: **the terminal takeover is a launch-flag bug on the integrating
side, not a defect to fix in pi.**

### 4.2 Optional light-touch upstream patch (belt-and-suspenders only)

If WG ever *does* want to embed pi's real TUI through a both-TTY PTY path **and** be
able to force-disable the grab from the outside (e.g. a shared launcher that can't
guarantee the flags), the minimal, default-off, upstreamable change is an env switch
at the top of `resolveAppMode` in `packages/coding-agent/src/main.ts`:

```ts
function resolveAppMode(parsed, stdinIsTTY, stdoutIsTTY) {
  // Default-off escape hatch: let a supervising harness force non-interactive
  // even when launched under a PTY (both fds are TTYs). No effect unless set,
  // and explicit --mode rpc/json still wins.
  if (process.env.PI_NO_TUI && parsed.mode === undefined && !parsed.print) {
    return "print";
  }
  if (parsed.mode === "rpc")  return "rpc";
  if (parsed.mode === "json") return "json";
  if (parsed.print || !stdinIsTTY || !stdoutIsTTY) return "print";
  return "interactive";
}
```

Properties that make it upstreamable: **default-off** (only active when `PI_NO_TUI`
is set), ~3 lines, no behavior change for existing users, composes with (does not
override) explicit `--mode`. It is strictly a convenience over "pass `-p`"; WG
should not block on it.

> **Bottom line:** ship the `pi:` handler with the §3 contract using `--mode rpc`
> (chat) / `-p`|`--mode json` (worker) and piped stdio. Treat the `PI_NO_TUI` patch
> as optional and only if a future both-TTY embed needs an external kill-switch.

---

## 5. Suggested follow-up (for `pi-design-integration` and implementation tasks)

Before first-class status, add these as `## Validation`-bearing tasks/smoke scenarios:
1. `wg config lint` rejects `pi:` when no `pi` binary/model is installable (B6).
2. RPC handler: launch `pi --mode rpc`, `get_state` → success, send a `prompt`
   against a real test provider, assert an `agent_end` and a non-empty
   `get_last_assistant_text` (B8).
3. **Takeover regression guard:** spawn pi under a PTY (e.g. `script`/`expect`) with
   `--mode rpc` and assert it does **not** enter raw mode / the process exits on
   SIGTERM within a timeout (locks in the §1 fix).
4. Process kill/restart resumes the same `--session-id` session.
5. Large bash/tool output yields structured truncation or a stable `fullOutputPath`.
6. One-shot `-p`/`--mode json` worker: prompt in, reply captured to
   `raw_stream.jsonl`/outbox, nonzero exit on credential error.

---

## Sources / artifacts examined

- pi source (read locally): `@earendil-works/pi-coding-agent@0.79.4` —
  `dist/cli.js`, `dist/main.js` (`resolveAppMode` 77-88, dispatch 399-403, 595-660),
  `dist/modes/print-mode.js`, `dist/modes/interactive/interactive-mode.js`
  (`ui.start()` 468, signal handling 2820-2887), `dist/config.js` (env names 388-390),
  `docs/rpc.md`, `docs/json.md`, `docs/models.md`, `docs/providers.md`.
- pi TUI driver: `@earendil-works/pi-tui@0.79.9` — `dist/terminal.js`
  (`start()` 74-101 incl. `setRawMode(true)` :80; `stop()`/restore 340-360;
  no `\x1b[?1049h` anywhere — verified by grep).
- WG side: `src/dispatch/handler_for_model.rs`, `src/dispatch/plan.rs`
  (`ExecutorKind`, `EXTERNAL_CLIS`, `WORKER_ONLY_EXTERNALS`),
  `src/commands/opencode_handler.rs`, `src/commands/spawn/execution.rs`,
  `src/tui/pty_pane.rs` (portable-pty embed), `src/executor_discovery.rs`.
- Prior report: `docs/reports/evaluate-pi-as-wg-executor.md` (task `evaluate-pi-as`).
- Upstream: https://github.com/earendil-works/pi · https://pi.dev/docs/latest
</content>
</invoke>
