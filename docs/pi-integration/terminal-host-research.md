# Generic Terminal-Host / Interactive-Child Host for WG — Research + Design

**Task:** `wg-terminal-host-research` · **Date:** 2026-06-22
**Status:** **Investigation only — no production code changed.**

This document generalizes the pi-specific terminal-takeover finding into a
**reusable WG layer** for hosting interactive, terminal-grabbing child tools
(pi, the `claude` CLI, the `codex` CLI, `opencode`, `aider`, `octomind`,
`dexto`, and future tools) inside WG's TUI and inside long-running
orchestration. It answers three things:

1. **The general problem** — a child wants raw-mode / PTY / full-screen while WG
   also owns (or wants to own) the terminal — and the **modes** WG must support.
2. **A survey** of approaches (PTY allocation/multiplexing, terminal-mode
   arbitration, session libraries, how tmux and other agent runners solve it)
   **and a precise inventory of the spawn/PTY infra WG already has.**
3. **A concrete generic terminal-host design** — interfaces + PTY strategy —
   that *any* executor can use with no per-tool plugin, plus how the pi-plugin
   path relates (plugin = no PTY fight; this layer = the fallback for pi and the
   primary mechanism for every other tool).

**Inputs synthesized:**
- [`executor-research.md`](executor-research.md) — pi terminal-takeover root
  cause (`resolveAppMode` → `setRawMode(true)`), the four pi runtime modes, and
  the wrapper-vs-patch verdict. The takeover mechanism it pins down is the
  *canonical example* of the generic problem this doc generalizes.
- [`integration-plan.md`](integration-plan.md) — the pi executor-handler design
  (`ExecutorKind::Pi`, `wg pi-handler`) and the chat/TUI two-shape split. This
  doc factors the *terminal-hosting* part of that plan out of pi and makes it
  generic.
- [`model-mgmt-research.md`](model-mgmt-research.md) — warm vs cold handler
  lifecycle (referenced where host lifetime matters).

**WG code anchors verified for this design (this branch):**
- `src/tui/pty_pane.rs` — `PtyPane` (4456 lines). `spawn`/`spawn_in` (`:139`,
  `:153`, direct `portable-pty` child); `spawn_via_tmux` (`:388`, detached tmux
  session + attach client); reader thread + `vt100::Parser` (`:205`, `:254`);
  capability-query responder (`:243-249`); `resize` threads parser + master
  (`:886-919`); `send_key`/`send_text`/`interrupt_foreground` (`:793`, `:839`,
  `:823`); `kill`/`Drop` (`:949`, `:965`); growth-rate guard (`:55-58`);
  `WG_PTY_DUMP` input/output tees (`:216-242`); tmux helpers (`:990-1101`).
- `src/tui/viz_viewer/state.rs:1391` `executor_uses_child_scroll_keys` (the
  per-executor alt-screen quirk switch); `:1405` `normalize_model_for_executor`;
  `:1729` `build_codex_chat_pty_args`; `:1776` `build_nex_chat_pty_args`;
  `:14776` chat-spawn tmux-session decision.
- `src/commands/spawn/execution.rs:653-655` worker `Stdio::null()` ×3; `:660-668`
  `setsid()` detach; `:573-586` wrapper-script spawn; `:1720-1773`
  `write_wrapper_script` (stdout → `raw_stream.jsonl` + `output.log`).
- `src/commands/opencode_handler.rs` — RPC/inbox handler: piped stdio
  (`:405-407`), stdout-is-protocol (`:20-25`), reply extraction (`:358-447`).
  Peers: `src/commands/claude_handler.rs`, `src/commands/codex_handler.rs`.
- `src/commands/exec.rs:398-400` `Stdio::inherit()` ×3 (raw outer-terminal
  handoff); `src/commands/spawn_task.rs:306-309` `execvp` process-replacement
  handoff so the PTY parent sees the handler's bytes directly.
- `src/commands/tui_pty.rs` / `src/commands/tui_nex.rs` — standalone full-screen
  PTY hosts (single child wrapped in a ratatui shell).
- `src/tui/viz_viewer/mod.rs:162-167` TUI teardown (`disable_raw_mode` +
  `LeaveAlternateScreen`) — the outer terminal state WG must save/restore around
  any handoff.

---

## 0. Executive summary

1. **The terminal-takeover problem is generic, not pi-specific.** Any
   interactive CLI that calls `setRawMode`/`tcsetattr(RAW)`, enables the
   alternate screen, or negotiates keyboard protocols will fight WG for the
   terminal the moment it is handed a TTY with no "be headless" flag. pi is one
   instance (`resolveAppMode` → `interactive` → `setRawMode(true)`,
   executor-research §1); `claude`/`codex`/`opencode`/`aider` are others. The
   fix must be a **WG-owned hosting layer**, not N per-tool patches (§1).
2. **WG already has all four hosting primitives — but as four disjoint,
   per-executor code paths with no common interface** (§2): the `PtyPane` TUI
   embed (direct + tmux-wrapped), the headless detached worker spawn, the
   piped RPC/inbox handlers, and the raw `inherit`/`execvp` handoff. Each new
   tool today means new bespoke branches (`executor_uses_child_scroll_keys`,
   `build_*_chat_pty_args`, a new `*_handler.rs`). That is the thing to unify.
3. **WG must support five hosting modes** (§3): (a) **embed** an interactive
   child in a TUI pane; (b) **headless/detached** long-running execution; (c)
   **handoff** — give the real terminal to the child, then take it back; plus
   two WG already leans on — (d) **protocol** (piped JSONL/RPC, no PTY) and (e)
   **standalone PTY host** (one child, full window). (c) is the least
   first-class today and the main gap.
4. **Recommended design: a `HostedChild` spec + a `TerminalHost` trait with one
   method per mode, driven by a declarative per-tool `TerminalProfile`** (§4).
   The executor declares *what kind of terminal citizen the tool is*
   (line-oriented vs alt-screen, raw-grabbing, RPC-capable, exits-on-error,
   resumable) and the host picks the PTY strategy. No tool-specific control flow
   leaks into the host; the per-executor `match` arms collapse into profile data.
5. **PTY strategy: keep `portable-pty` + `vt100` + tmux-wrapping; promote them
   behind the trait.** The single highest-value piece WG already proved out — the
   **capability-query responder** (answering DA/XTVERSION/DECRQM so `claude`
   doesn't freeze post-splash, `pty_pane.rs:243-249`) — becomes a host
   guarantee every embedded tool inherits for free (§4.4).
6. **Relationship to the pi plugin: orthogonal and complementary** (§5). A pi
   *plugin* (pi calling into WG over its own protocol) means **no PTY fight at
   all** — best case when it exists. The terminal-host is **(i)** the fallback
   for pi when the plugin is absent/unsuitable and **(ii)** the *primary*
   mechanism for every tool that will never have a WG plugin (`claude`/`codex`
   CLIs, `aider`, arbitrary REPLs). **WG-spawns-pi-as-a-worker is itself a
   first-class terminal-host use case**, independent of any plugin.
7. **This is investigation only.** §6 lists the implementation tasks to spawn
   after review (extract the trait, port the three existing executors onto it,
   add the handoff mode, profile-drive the quirks).

---

## 1. The general problem

### 1.1 Statement

> A child process wants **exclusive, low-level control of a terminal** — raw
> mode (line discipline off: no echo, no canonical buffering, **no ISIG**, so
> Ctrl-C/Ctrl-Z stop becoming signals), often the **alternate screen**, often a
> **keyboard-protocol negotiation** (Kitty / `modifyOtherKeys` / DA /
> XTVERSION / bracketed paste). At the same time **WG owns, or wants to own,
> the same terminal** — either because WG is drawing its own ratatui TUI on it,
> or because WG is a long-running daemon that must capture the child's output as
> structured data instead of as screen paint.

When both sides try to drive one terminal, the symptoms are exactly what the pi
report documented (executor-research §1.3): wrecked scrollback, keys not
echoing, dead Ctrl-C, and stray capability-query warnings (`tmux extended-keys
is off…`). Critically, the pi report establishes that the takeover is **not a
defect of the tool** — it is a **launch-context property**: a tool takes over
**iff** it is given a TTY on the relevant fds with no "headless" flag. The same
binary is perfectly well-behaved when handed pipes or `-p`. The generic problem
is therefore **how WG controls that launch context, per tool, per mode**, behind
one interface.

### 1.2 Why it is generic (not pi-only)

The behaviors that cause the fight are standard terminal-app machinery, not pi
inventions:

| Behavior | pi | claude CLI | codex CLI | opencode | aider / generic REPL |
|---|---|---|---|---|---|
| raw mode (`setRawMode`/`tcsetattr`) | yes (`terminal.js:80`) | yes | yes | yes | usually |
| alternate screen (`\x1b[?1049h`) | **no** (inline repaint) | no (line) | no (line) | **yes** | varies |
| keyboard-proto / DA / XTVERSION queries | yes (Kitty) | **yes** (blocks until answered) | yes | yes | varies |
| exits non-zero on cred error when headless | yes (`-p`) | yes | yes | yes | varies |
| sits forever in TUI on error when interactive | yes (B2) | yes | yes | yes | yes |

Two of these are load-bearing for WG and already special-cased per tool today:
- **DA/XTVERSION/DECRQM queries that block the child until answered.** WG's
  reader thread already answers them (`pty_pane.rs:243-249`) — *added because
  `claude` froze post-splash without it.* Every embedded raw-mode tool needs
  this; today it lives in one place but is reasoned about per tool.
- **alt-screen vs inline.** Alt-screen tools (`opencode`) have no tmux
  copy-mode scrollback to walk, so WG forwards the child's *own* scroll keys
  instead (`executor_uses_child_scroll_keys`, `state.rs:1391`). Line-oriented
  tools (`claude`/`codex`/`nex`/pi-inline) use tmux copy-mode. **This is a
  per-tool terminal property** — exactly the kind of thing a profile should
  carry instead of a hard-coded `executor == "opencode"`.

The conclusion the pi research reached for one tool — *control the launch
context, don't patch the tool* — is the right **general** posture. What is
missing is the **abstraction** so it is stated once and reused, instead of
re-derived for pi, then aider, then the next tool.

### 1.3 The modes WG needs (the task's (a)/(b)/(c), expanded)

| # | Mode | What WG owns | What the child gets | Today |
|---|---|---|---|---|
| **a** | **Embed** in a TUI pane | the outer terminal + a ratatui frame | its own *private* PTY, rendered into a pane | `PtyPane` (direct / tmux) ✅ |
| **b** | **Headless / detached** long-run | nothing visual; captures output as data | null/file stdio, no TTY → tool self-selects headless | worker spawn ✅ |
| **c** | **Handoff** (give → take back) | the outer terminal, lent then reclaimed | the *real* terminal, full control, temporarily | partial: `exec`/`execvp` give; **take-back not first-class** ⚠️ |
| **d** | **Protocol** (no PTY) | a JSONL/RPC channel | piped stdin/stdout; never a TTY | RPC handlers ✅ |
| **e** | **Standalone PTY host** | the whole window as a host shell | a private PTY filling the window | `tui_pty`/`tui_nex` ✅ |

The task names (a), (b), (c). (d) and (e) already exist and the generic layer
must not regress them — they are the *terminal-free* and *single-child* ends of
the same spectrum and belong in the same taxonomy so executors choose a mode
rather than a code path. **(c) is the real gap**: WG can *give* the terminal
away (`exec.rs` `Stdio::inherit`, `spawn_task.rs` `execvp`) but has no clean,
reusable "suspend my TUI → run the child on the real terminal → restore my TUI"
take-back primitive; the closest reusable take-back today is tmux
**detach/reattach** (a child in a detached session, §2.1).

---

## 2. What WG already has (inventory)

WG has **four** child-hosting code paths plus two standalone hosts. They cover
the five modes — but each is bespoke and there is **no common interface**.

### 2.1 TUI PTY embed — `src/tui/pty_pane.rs` (mode a; tmux variant → mode c take-back)

The richest piece, and the natural seed for the generic host. `PtyPane`:

- **Two spawn strategies behind one struct:**
  - `spawn`/`spawn_in` (`:139`/`:153`) — a **direct** child on a `portable-pty`
    PTY pair (both ends are a pseudo-terminal). Drops the slave in the parent so
    EOF propagates on child exit (`:187-190`).
  - `spawn_via_tmux` (`:388`) — creates a **detached** `tmux new-session -d`,
    applies WG-owned session options (`apply_session_options`, `:454`), then the
    pane is a `tmux attach -d` client running on a direct PTY. **The child lives
    in the tmux session, not as WG's direct child** — so dropping the pane kills
    only the attach client; the session (and the agent) survives
    (`:97-102`, `kill_underlying_session` `:478`). **This is WG's existing
    give-and-take-back primitive**: WG can detach and a human can `tmux attach`,
    or WG can reattach later.
- **Render pipeline:** a dedicated reader thread (`:254`) drains the master into
  an `Arc<Mutex<vt100::Parser>>` (`:205`); the TUI takes a read lock on render
  (`tui_term`/`PseudoTerminal`) and a write lock for `send_key`/`send_text`.
- **Capability-query responder (the crown jewel):** the reader thread peeks raw
  PTY bytes for DA/XTVERSION/DECRQM and writes replies through the shared writer
  (`:243-249`) — without this, `claude` freezes post-splash waiting for answers.
- **Resize correctness:** `resize` (`:886`) pushes new dims to **both** the
  parser `set_size` **and** the master PTY (`:913`) so the child gets SIGWINCH
  and reflows; includes scrollback-reflow handling to avoid duplicate lines.
- **Per-executor quirks already living here:** `set_child_scroll_keys`/
  `uses_child_scroll_keys` (`:758`/`:765`) and the alt-screen vs copy-mode
  decision via `executor_uses_child_scroll_keys` (`state.rs:1391`); DEC-2026
  synchronized-output trimming for `codex`'s animated repaints (`:262-270`).
- **Safety:** growth-rate guard (discard scrollback above 512 KB/s to avoid OOM,
  `:55-58`); `WG_PTY_DUMP` input+output tees for smoke-testing key forwarding
  (`:216-242`); `interrupt_foreground` for Ctrl-C semantics (`:823`).

### 2.2 Headless detached worker — `src/commands/spawn/execution.rs` (mode b)

- All three fds set to `Stdio::null()` (`:653-655`) → neither stdin nor stdout
  is a TTY → a well-behaved tool self-selects its headless mode (pi `print`,
  `claude -p`, etc.). This is the **mode-b application of the §1.1 insight**: by
  withholding a TTY, WG defeats the takeover without any tool cooperation.
- `setsid()` via `pre_exec` (`:660-668`) detaches the child into its own
  session/process group so it survives daemon restart/crash.
- A generated **wrapper script** (`write_wrapper_script`, `:1720`) runs the real
  command and captures stdout → `raw_stream.jsonl` (+ tee to `output.log`),
  stderr → `output.log` (`:1756-1773`), and appends `wg done` bookends.

### 2.3 RPC / inbox handlers — `opencode_handler.rs` & peers (mode d)

- Piped stdio (`opencode_handler.rs:405-407`), **never a PTY**; a JSONL/RPC
  protocol over stdin/stdout. Strict **stdout-is-protocol** discipline:
  diagnostics go to `handler.log`/stderr only (`:20-25`), enforced by
  `tests/integration_handler_stdout_pristine.rs`.
- Reply extraction + model normalization (`:358-447`, `:273-299`). Peers:
  `claude_handler.rs`, `codex_handler.rs`. This is where the pi plan adds a
  fourth, `pi_handler.rs` — *more bespoke duplication this doc wants to curb.*

### 2.4 Raw outer-terminal handoff — `exec.rs` / `spawn_task.rs` (mode c, give-only)

- `exec.rs:398-400` sets `Stdio::inherit()` ×3 and runs the child to completion
  on **WG's real terminal** — the child gets the actual TTY directly.
- `spawn_task.rs:306-309` goes further: on Unix it **`execvp`-replaces** the WG
  process with the handler so "the PTY parent sees the handler's bytes
  directly" — a zero-overhead handoff with no take-back (WG is gone).
- **Gap:** there is no reusable "WG TUI is up → temporarily lend the real
  terminal to a child → restore the TUI afterward" path. Doing it correctly
  requires the save/restore that the TUI teardown already performs once at exit
  (`viz_viewer/mod.rs:162-167`: `disable_raw_mode` + `LeaveAlternateScreen` +
  `DisableBracketedPaste`) — but that logic is not packaged for transient
  handoff. tmux-wrapping (§2.1) is the only reusable take-back today.

### 2.5 Standalone PTY hosts — `tui_pty.rs` / `tui_nex.rs` (mode e)

Full-screen ratatui shells that host a single PTY child filling the window (own
`enable_raw_mode`/`EnterAlternateScreen` setup + teardown). These are
mode-e degenerate cases of the embed (one pane = whole screen) and should sit in
the same taxonomy.

### 2.6 The shared gap

Every path re-implements stdio wiring, lifetime, and quirks. Adding a tool
means: a new `ExecutorKind`, a new `*_handler.rs`, a new `build_*_chat_pty_args`,
and possibly a new `executor_uses_child_scroll_keys` branch. **There is no
`TerminalHost` interface and no declarative per-tool terminal profile.** That is
exactly what makes "now do it for pi" (and next for aider) an N× effort instead
of a data entry.

---

## 3. Survey of approaches

### 3.1 PTY allocation + multiplexing
- **A private PTY per child** (`portable-pty`, what WG uses) is the right
  primitive for mode a/e: the child gets a real TTY (so it behaves naturally and
  its raw-mode grab is **contained to that PTY**, never WG's real terminal), and
  WG reads the master as a byte stream into a `vt100` screen model. Cost: WG must
  emulate a terminal (vt100 + tui-term) and answer capability queries.
- **Multiplexing many children** onto one render surface = WG's pane model;
  `vt100` per child + ratatui layout already does this. The growth-rate guard
  (`:55-58`) is the back-pressure mechanism.

### 3.2 Terminal-mode arbitration
- The crux from §1: **arbitration = controlling the launch context.** Three
  levers, all already used somewhere in WG: (1) **withhold a TTY** (pipes/null →
  headless, mode b/d); (2) **pass a headless flag** (`-p`/`--mode rpc` — pi
  executor-research §1.1); (3) **give a private PTY** so the grab is real but
  sandboxed (mode a/e). A generic host encodes *which lever per mode* once.
- **DA/XTVERSION/DECRQM answering** is a sub-problem of arbitration WG already
  solved (`:243-249`). Generalize it as a host guarantee.

### 3.3 Session libraries / detach
- **tmux as the session substrate** (WG's `spawn_via_tmux`) buys
  detach/reattach, persistence across WG restarts, and a human escape hatch
  (`tmux attach`) **for free**, at the cost of a tmux dependency and copy-mode
  quirks for alt-screen apps. WG already depends on it for chat panes
  (`state.rs:14776`) and degrades to direct-PTY when tmux is absent
  (`warn_chat_tmux_missing_once`).
- **`portable-pty` direct** is the no-dependency fallback (no detach/persistence).
- The generic host should treat **tmux-backed vs direct** as a *backend choice*
  under the same trait, exactly as `PtyPane` does today across its two
  constructors — not as two different APIs.

### 3.4 How others solve it
- **tmux/screen:** own the real terminal, give each child a private PTY, render a
  chosen pane, multiplex input. WG's embed is a single-pane special case of this.
- **VS Code / Zellij / wezterm:** PTY-per-terminal + a vt parser + a renderer;
  identical shape to `PtyPane`.
- **Agent runners (claude-squad, aider wrappers, octomind/dexto prototypes):**
  most either (i) run the tool headless and scrape structured output (mode b/d),
  or (ii) drop the tool into tmux and attach (mode a via §2.1). WG already does
  both — the contribution here is unifying them, not inventing a new mechanism.

### 3.5 Takeaway
WG does **not** need new terminal technology. It needs to **lift its four
existing paths behind one interface** and add the missing **transient handoff
take-back** (mode c). The pi research's "control the launch flags" is the
arbitration policy; this design is the housing for it.

---

## 4. Recommended generic terminal-host design

### 4.1 Shape

Three pieces, all WG-side, no per-tool plugin required:

1. **`HostedChild`** — a spec describing *what to run* (command, args, env, cwd,
   credentials-by-env, session id) independent of *how* it is hosted.
2. **`TerminalProfile`** — declarative data describing *what kind of terminal
   citizen the tool is*. This replaces the scattered `match executor` arms.
3. **`TerminalHost`** — a trait with **one method per mode** (a–e). Executors
   pick a method; the host applies the right launch context + PTY backend.

```rust
/// What to run — independent of how it is hosted.
pub struct HostedChild {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,     // credentials by env, never argv (B5)
    pub cwd: Option<PathBuf>,
    pub session_id: Option<String>,     // → tmux session name / --session-id
}

/// What kind of terminal citizen the tool is. Pure data — the single
/// source of truth that today is spread across executor_uses_child_scroll_keys,
/// build_*_chat_pty_args, and the handler files.
pub struct TerminalProfile {
    /// Tool takes the alternate screen (opencode) vs renders inline/line
    /// (claude/codex/nex/pi). Drives scroll strategy: child-scroll-keys
    /// vs tmux copy-mode (replaces executor_uses_child_scroll_keys).
    pub alt_screen: bool,
    /// Tool emits DA/XTVERSION/DECRQM and blocks until answered (claude).
    /// If true, the host's capability responder is mandatory.
    pub needs_capability_replies: bool,
    /// How to force headless: a flag (e.g. "-p", or ["--mode","rpc"]) and/or
    /// "just withhold a TTY". None ⇒ withholding a TTY is sufficient.
    pub headless_flag: Option<Vec<String>>,
    /// Tool speaks a line-delimited JSONL/RPC protocol when headless
    /// (pi --mode rpc, opencode) ⇒ eligible for mode d.
    pub rpc_capable: bool,
    /// Tool exits non-zero on error when headless (so a supervisor timeout
    /// + exit code is a reliable failure signal). Interactive mode does not.
    pub exits_on_error_headless: bool,
    /// Animated full-screen repaints bracketed by DEC-2026 sync (codex) ⇒
    /// enable the sync-mode scrollback trim.
    pub sync_mode_repaints: bool,
}

pub enum HostError { Spawn(..), Timeout, NonZeroExit(i32), TmuxUnavailable, .. }

pub trait TerminalHost {
    /// (a) Embed in a TUI pane: private PTY, rendered into `area`.
    ///     Backend = tmux-wrapped if available & session_id set, else direct.
    fn embed(&mut self, child: HostedChild, profile: &TerminalProfile,
             size: PtySize) -> Result<PaneHandle, HostError>;

    /// (b) Headless / detached long-run: null/file stdio, setsid, wrapper
    ///     capture. Returns once spawned; output lands in the run dir.
    fn run_headless(&mut self, child: HostedChild, profile: &TerminalProfile,
                    capture: CaptureSpec) -> Result<DetachedHandle, HostError>;

    /// (c) Handoff: lend the real terminal, run to completion, restore.
    ///     Saves/restores WG's outer terminal state around the child.
    fn handoff(&mut self, child: HostedChild, profile: &TerminalProfile,
               term: &mut OuterTerminal) -> Result<ExitStatus, HostError>;

    /// (d) Protocol: piped stdio, no PTY; returns a framed JSONL channel.
    fn open_protocol(&mut self, child: HostedChild, profile: &TerminalProfile)
        -> Result<RpcChannel, HostError>;

    /// (e) Standalone PTY host: one child filling the window (mode-a with
    ///     area = whole screen + own ratatui shell). Thin wrapper over embed.
    fn host_fullscreen(&mut self, child: HostedChild, profile: &TerminalProfile)
        -> Result<ExitStatus, HostError>;
}
```

`PaneHandle` exposes the existing `PtyPane` surface (`send_key`, `send_text`,
`interrupt_foreground`, `resize`, `scroll_*`, `render`, `is_alive`, `kill`) so
the TUI is unchanged above the trait. `DetachedHandle` mirrors the worker spawn
result (pid, run dir, exit polling).

### 4.2 How each existing path maps onto the trait (port plan, no rewrite)

| Mode | Trait method | Backed by today's code |
|---|---|---|
| a | `embed` | `PtyPane::spawn_via_tmux` / `spawn_in` (`pty_pane.rs`) |
| b | `run_headless` | `execution.rs:653-668` + `write_wrapper_script` |
| c | `handoff` | `exec.rs` inherit **+ new** save/restore around it (§4.3) |
| d | `open_protocol` | `opencode_handler.rs` framing (LF-only `read_until`) |
| e | `host_fullscreen` | `tui_pty.rs` / `tui_nex.rs` |

The profile *replaces* the per-executor branches: `alt_screen` subsumes
`executor_uses_child_scroll_keys` (`state.rs:1391`); `needs_capability_replies`
formalizes the responder (`pty_pane.rs:243-249`); `sync_mode_repaints` gates the
DEC-2026 trim; `headless_flag` is what the worker/RPC paths pass.

### 4.3 The one genuinely new piece: transient handoff (mode c take-back)

`handoff` is the gap (§2.4). It must:
1. **Save** WG's outer terminal state and tear the TUI down to a clean cooked
   terminal — reuse the exit sequence at `viz_viewer/mod.rs:162-167`
   (`disable_raw_mode`, `LeaveAlternateScreen`, `DisableBracketedPaste`),
   packaged as a reusable `OuterTerminal::suspend()`.
2. **Run** the child with `Stdio::inherit()` (as `exec.rs:398-400` already does)
   to completion. The child owns the real terminal; its raw-mode grab is now
   legitimate and uncontested.
3. **Restore** WG's TUI (`enable_raw_mode`, `EnterAlternateScreen`, re-enable
   bracketed paste, force a full repaint / SIGWINCH) — `OuterTerminal::resume()`.

For the **detach/reattach** flavor of take-back (long-lived child, WG comes and
goes), prefer the existing **tmux-wrapped** path (§2.1): the child stays in a
detached session; `embed` reattaches. `handoff` is for the *synchronous lend*
("drop me into `claude` interactive, come back when it exits"); tmux-wrap is for
the *asynchronous lend*. Both are first-class methods/back-ends, not ad hoc.

### 4.4 PTY strategy (decision)

- **Keep `portable-pty` + `vt100` + `tui-term`.** They already work and isolate
  the child's raw-mode grab to a private PTY (§3.1). No new dependency.
- **tmux-wrapped is the default `embed`/take-back backend when tmux is present
  and a `session_id` is set; direct PTY is the fallback** — exactly today's
  behavior (`state.rs:14776` + `warn_chat_tmux_missing_once`), now expressed as a
  backend choice under `embed`, not a separate API.
- **Make the capability-query responder a host guarantee** keyed on
  `profile.needs_capability_replies`, so every raw-mode embed inherits the
  anti-freeze fix `claude` needed — no per-tool rediscovery.
- **Headless = withhold a TTY (+ optional `headless_flag`).** This is the §1.1
  arbitration policy as code: mode b/d never give a PTY, so no tool can take
  over; `headless_flag` makes intent explicit and robust where the tool supports
  it (pi `--mode rpc`, `claude -p`).
- **Credentials by env, never argv** (executor-research B5) — enforced in
  `HostedChild` construction for all modes.

### 4.5 What this buys WG

- **Adding a tool becomes data entry:** define a `TerminalProfile`, point an
  `ExecutorKind` at it. No new `build_*_chat_pty_args`, no new
  `executor_uses_child_scroll_keys` arm, often no new handler file.
- **One place to fix terminal bugs:** capability replies, resize/reflow,
  sync-mode trim, growth guard — fixed once, inherited by all.
- **Mode c stops being ad hoc:** `handoff` + tmux-reattach are first-class.
- **The pi handler shrinks:** `pi_handler.rs` becomes "fill in pi's
  `TerminalProfile` + RPC framing", reusing `open_protocol`/`embed`/`handoff`
  instead of re-deriving stdio wiring (§5).

---

## 5. Relationship to the pi plugin and to other tools

### 5.1 The two are orthogonal — and complementary

- **A pi *plugin*** (pi loading a WG extension and talking to WG over pi's own
  plugin/RPC channel) means **there is no terminal to fight over**: pi renders
  its own UI, and WG participates as a tool/provider inside pi. When that path
  exists and is chosen, the terminal-host is **not** on the critical path. This
  is the "plugin = no PTY fight" case.
- **The generic terminal-host** is what WG uses when **WG is the host and the
  tool is the guest** — i.e. the inverse direction. It is needed:
  1. **As pi's fallback** — when WG wants to *spawn and host pi* (embed pi in a
     WG pane, run pi as a headless worker, or hand pi the terminal) without
     requiring the plugin to be installed/active. **WG-spawns-pi-as-a-worker is
     a primary, plugin-independent use case**: pi grabs raw mode the instant it
     sees a TTY (executor-research §1), so WG must either give pi its **own
     managed PTY** (mode a/e — grab contained) or run it **fully headless**
     (mode b/d — no TTY, `--mode rpc`/`-p`). Both are terminal-host modes.
  2. **As the primary mechanism for every tool that will never have a WG
     plugin** — the `claude`/`codex` CLIs, `aider`, `opencode`, arbitrary REPLs.
     These can only ever be *hosted as guests*; the terminal-host is their sole
     integration route.

### 5.2 Decision matrix

| Situation | Use |
|---|---|
| pi available **and** pi-plugin path chosen, pi drives the UI | **pi plugin** (no terminal-host) |
| WG wants to embed pi in a WG TUI pane | terminal-host **mode a** (private PTY, contains the grab) |
| WG dispatches pi as a long-running headless worker | terminal-host **mode b/d** (no TTY / `--mode rpc`) |
| User wants to drop into interactive pi and return to WG | terminal-host **mode c** (`handoff`) |
| Hosting `claude`/`codex`/`aider`/any non-plugin tool | terminal-host (a/b/c/d/e as fit) — **only** option |

### 5.3 Consistency with the pi integration plan

The pi plan (`integration-plan.md`) adds `ExecutorKind::Pi` and a `pi_handler.rs`
with two shapes (RPC chat / one-shot worker). Under this design those shapes are
**`open_protocol` (d)** and **`run_headless` (b)** with a pi `TerminalProfile`
(`alt_screen=false`, `needs_capability_replies=true` for an embedded pi,
`rpc_capable=true`, `headless_flag=["--mode","rpc"]` or `["-p"]`,
`exits_on_error_headless=true`). The plan's *terminal-takeover avoidance* (launch
with `--mode rpc`/`-p` + piped stdio) is precisely the host's mode-b/d
arbitration policy (§4.4) — so building the generic host first makes the pi
handler thinner, and the pi handler is a faithful first consumer that validates
the trait. They do not conflict; the host is the layer *under* the pi handler.

---

## 6. Suggested follow-up tasks (documented, not spawned)

To be created after this research is human-reviewed (the
`.flip-wg-terminal-host-research` gate). Shared-file sequencing noted.

1. **`terminal-host-trait`** — introduce `HostedChild`, `TerminalProfile`,
   `TerminalHost` in a new `src/terminal_host/mod.rs`. No behavior change; trait
   + structs + a direct `portable-pty` impl that *delegates to* today's
   `PtyPane`. *Validation:* `embed` of a known tool renders identically to the
   current `PtyPane` path (PTY smoke scenario unchanged).
2. **`terminal-host-port-embed`** — make the TUI chat pane construct via
   `TerminalHost::embed` + a `TerminalProfile`, replacing
   `executor_uses_child_scroll_keys` and the `build_*_chat_pty_args` branches
   with profile data. *File scope:* `src/tui/viz_viewer/state.rs`,
   `src/terminal_host/`. *Validation (user-visible):* live PTY/tmux scenario —
   claude/codex/opencode/nex chat panes still scroll, resize, and forward keys
   (drive via tmux send-keys; do not regress existing smoke scenarios).
3. **`terminal-host-handoff`** — implement mode c: `OuterTerminal::suspend/
   resume` extracted from `viz_viewer/mod.rs:162-167`, and `handoff` using
   `Stdio::inherit` with save/restore. *Validation:* a scenario that suspends a
   running TUI, runs an interactive child to exit, and asserts the TUI restores
   raw mode + alt screen + repaint.
4. **`terminal-host-port-headless`** — route worker spawn (`execution.rs`) and
   the RPC handlers through `run_headless`/`open_protocol`, deduplicating stdio
   wiring. *Validation:* worker output still captured to `raw_stream.jsonl`;
   `tests/integration_handler_stdout_pristine.rs` still green.
5. **`terminal-host-pi-consumer`** — implement the pi handler (`integration-plan`
   P1a) **on top of** the trait (pi `TerminalProfile` + RPC framing), proving the
   host generalizes. *Validation:* the pi takeover-regression guard
   (executor-research §5.3) passes via the generic host.

These are sequential where they share `state.rs` (2 before 3) and otherwise
independent. Each is "implement directly — do not decompose further" and carries
a `## Validation` section per the WG contract (user-visible items use live
PTY/tmux flows, not CLI substitutes).

---

## 7. Validation of this research (task acceptance)

- [x] `docs/pi-integration/terminal-host-research.md` created (this file).
- [x] **Generic problem + required modes enumerated** — §1 (statement +
      genericity table) and §1.3 / §3 (modes a–e, mapped to the task's a/b/c).
- [x] **Concrete generic terminal-host design proposed** — §4
      (`HostedChild` / `TerminalProfile` / `TerminalHost` trait, PTY strategy,
      the new transient-handoff piece).
- [x] **Relationship to the pi-plugin path and other tools spelled out** — §5
      (orthogonal/complementary, decision matrix, consistency with the pi plan).
- [x] **Investigation only; no code changes** — only this doc is added.

---

## Sources

- [`docs/pi-integration/executor-research.md`](executor-research.md) — pi
  terminal-takeover root cause & wrapper verdict (the canonical instance of the
  generic problem).
- [`docs/pi-integration/integration-plan.md`](integration-plan.md) — pi
  executor-handler/chat design this doc factors the terminal layer out of.
- [`docs/pi-integration/model-mgmt-research.md`](model-mgmt-research.md) —
  warm/cold handler lifetime (host-lifetime context).
- WG code anchors enumerated in the header (verified this branch):
  `src/tui/pty_pane.rs`, `src/tui/viz_viewer/state.rs`,
  `src/commands/spawn/execution.rs`, `src/commands/opencode_handler.rs`,
  `src/commands/exec.rs`, `src/commands/spawn_task.rs`,
  `src/commands/tui_pty.rs`, `src/commands/tui_nex.rs`,
  `src/tui/viz_viewer/mod.rs`.
- Dependencies: `portable-pty = 0.9`, `vt100 = 0.16`, `ratatui = 0.30`,
  `crossterm = 0.29`, `tui-term` (Cargo.toml).
</content>
</invoke>
