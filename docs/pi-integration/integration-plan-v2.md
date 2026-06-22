# Pi.dev Integration Plan v2 — Plugin-First + Generic Terminal-Host (supersedes the wrapper P0–P5 plan)

**Task:** `pi-plugin-replan` · **Date:** 2026-06-22
**Status:** **Replan only — no production code changed.** Re-derives the
implementation task graph for the plugin-first decision. Reviewable before any
implementation starts (`.flip-pi-plugin-replan`).

This plan **supersedes** the wrapper-first
[`integration-plan.md`](integration-plan.md) for the *in-session* integration
surface, and folds in the
[`terminal-host-research.md`](terminal-host-research.md) generic layer as the
orthogonal takeover-fix and other-tools path. It is the v2 the
[`plugin-research.md`](plugin-research.md) verdict called for.

**Inputs synthesized:**
- [`plugin-research.md`](plugin-research.md) — pi has a **first-class, documented
  extension/plugin API today** (`docs/extensions.md`, `docs/sdk.md`, 77 examples).
  `registerTool`/`registerCommand`/`registerProvider`, `setModel`, `ctx.ui.*`,
  lifecycle hooks, `DefaultResourceLoader({extensionFactories,eventBus})`.
  Extensions load in **all four** pi modes (tui/rpc/json/print). Designs the
  `wg-pi-plugin` package, three deployment topologies, and a replaces-vs-keeps §5.
- [`terminal-host-research.md`](terminal-host-research.md) — the terminal-takeover
  problem is **generic**, not pi-specific; proposes a WG-owned
  `HostedChild`/`TerminalProfile`/`TerminalHost` layer (modes a–e) so any
  raw-mode child (pi, claude/codex CLIs, aider, opencode) is hosted behind one
  interface. Establishes that **plugin and terminal-host are orthogonal**.
- [`integration-plan.md`](integration-plan.md) — the prior wrapper-first plan
  (P0–P5). This v2 keeps its WG-side routing/profile/verb scaffolding and replaces
  its in-session mechanism (prompt-munging → plugin).
- Revised user guidance on `pi-impl-p5` (queued to this task, 2026-06-22): the
  plugin removes terminal-takeover **only** for the human-launches-pi case; for
  WG-spawns-pi-as-worker the takeover persists and must be fixed by **either** the
  generic terminal-host (managed PTY) **or** a pi headless mode
  (`PI_NO_TUI`/`--mode`). Treat **plugin = integration channel** and
  **terminal-host-or-headless = takeover fix** as **orthogonal, both required**.

**WG code anchors (verified this branch):**
- `src/dispatch/plan.rs` — `ExecutorKind` (`:48`), `EXTERNAL_CLIS` (`:90`),
  `WORKER_ONLY_EXTERNALS` (`:109`), `parse_executor_model_route` (`:221`).
  **`ExecutorKind::Pi` already landed** (pi-impl-p0-executor-kind, done).
- `src/dispatch/handler_for_model.rs` — external-CLI prefix interception (a
  `pi:` spec routes with no new match arm).
- `src/executor_discovery.rs` — `EXPERIMENTAL_EXTERNAL_EXECUTORS` (pi added by p0).
- `src/commands/opencode_handler.rs` (+ `claude_handler.rs`, `codex_handler.rs`) —
  RPC/inbox handler template (piped stdio, stdout-is-protocol, inbox-cursor loop).
- `src/commands/service/ipc.rs` — `SetChatExecutor` (`:602`),
  `handle_set_coordinator_executor` (`:1909`, persist override → SIGTERM → respawn).
- `src/nex_runtime.rs` — nex binds model once at construction.
- `src/tui/pty_pane.rs` — `PtyPane` (direct `portable-pty` + `spawn_via_tmux`),
  the capability-query responder (`:243-249`), resize/reflow.
- `src/tui/viz_viewer/state.rs` — `executor_uses_child_scroll_keys` (`:1391`),
  `build_*_chat_pty_args`; `mod.rs:162-167` TUI teardown (suspend/resume seed).
- `src/profile/templates/{claude,codex,nex,opencode}.toml` — starter templates.

---

## 0. Executive summary

1. **Plugin-first: ALL pi-backed wg tasks and chat go through a pi-side `wg`
   plugin.** The integration lives *inside* a pi session as a loaded module
   (`@worksgood/wg-pi-plugin`), the same artifact whether a human launched pi or
   WG did. WG stops prompt-munging/scraping pi as a subordinate CLI; instead the
   plugin registers wg tools/commands, surfaces the task graph, and bridges model
   state — natively, inside pi's lifecycle (`plugin-research.md` §1).
2. **`ExecutorKind::Pi` is kept, but routes *through the plugin*, not CLI-spawn
   prompt-munging.** "Routes through the plugin" = WG launches a pi runtime that
   has the plugin loaded; all in-session wg-awareness comes from the plugin. WG is
   Rust and cannot host a TS extension in-process, so this is realized by *what WG
   spawns* — `pi --mode rpc` with the plugin present (Topology A) or a Node SDK
   host embedding the plugin (Topology B) (`plugin-research.md` §4).
3. **Plugin and takeover-fix are ORTHOGONAL and BOTH required.** The plugin is
   the *integration channel*; it does **not** change pi's invocation/mode and
   therefore does **not** prevent the terminal takeover when WG spawns pi as an
   unattended worker. The takeover for that path is fixed independently by the
   **generic terminal-host** (give pi a managed PTY, or run it headless) **or** a
   **pi headless mode** (`--mode rpc`/`-p`, optionally the staged `PI_NO_TUI`).
   Both axes ship; neither subsumes the other (`plugin-research.md` §3, revised
   user guidance).
4. **The generic terminal-host is the fallback and the other-tools path.** When
   the plugin is absent/unsuitable, WG hosts pi as a guest via the terminal-host
   (mode a/e managed PTY, or mode b/d headless). For every tool that will never
   have a WG plugin (`claude`/`codex` CLIs, `aider`, arbitrary REPLs) the
   terminal-host is the *only* integration route (`terminal-host-research.md` §5).
5. **Warm mid-chat `set_model` becomes native via the plugin.** Pi's warm
   in-process model switch (`setModel`, no respawn) is now the *normal* path for a
   pi/plugin session — promoted from the wrapper plan's "optional Layer 2." The
   plugin subscribes to `model_select` and writes the choice back into WG's
   `CoordinatorState.model_override`. The **nex** per-turn warm swap is
   pi-independent and kept as its own (still-optional) task.
6. **Do NOT rebase WG's default stack around pi.** Unchanged from
   `integration-plan.md` §4: adopt pi **additively** (plugin + terminal-host both
   gate behind `EXPERIMENTAL_EXTERNAL_EXECUTORS` until the credentialed pass is
   green). Node + the plugin bundle is a hard prerequisite *only* for the pi
   handler, acceptable for an experimental handler, not for a default rebase.

---

## 1. Architecture: the two orthogonal axes

```
                       ┌──────────────────────────────────────────────┐
                       │  AXIS 1 — INTEGRATION CHANNEL (the plugin)      │
                       │  @worksgood/wg-pi-plugin loaded inside pi:      │
                       │  • wg tools (LLM/human call wg verbs)           │
                       │  • /wg, /wg-model commands + task-graph widget  │
                       │  • registerProvider(WG endpoints/keys)          │
                       │  • setModel + model_select → CoordinatorState   │
                       │  Loads in ALL pi modes (tui/rpc/json/print).    │
                       └──────────────────────────────────────────────┘
                                          ⟂  (orthogonal — neither implies the other)
                       ┌──────────────────────────────────────────────┐
                       │  AXIS 2 — TAKEOVER FIX (WG-spawns-pi-as-worker) │
                       │  pi grabs the terminal iff handed a both-TTY    │
                       │  context with no mode flag. Fix, independently: │
                       │   (a) generic terminal-host: managed PTY (mode  │
                       │       a/e) OR headless null/pipe stdio (b/d)     │
                       │   (b) pi headless mode: --mode rpc/-p, optional │
                       │       PI_NO_TUI (staged, pi-impl-p5)            │
                       └──────────────────────────────────────────────┘
```

**Why orthogonal (the load-bearing insight, `plugin-research.md` §3):** the
plugin is code pi loads *after* `resolveAppMode` has already decided whether to
grab the terminal. Loading a plugin neither causes nor prevents
`setRawMode(true)`. So:

- **Direction (1) — human hosts pi (Topology C):** the human runs `pi`
  interactively; the plugin is auto-discovered. Full-screen takeover is *the
  product* (pi's native TUI + `/wg` + wg tools + graph widget). WG spawns nothing.
  **The plugin alone fully delivers the integration here; takeover is desired.**
- **Direction (2) — WG spawns pi as worker/chat handler:** pi still runs
  `resolveAppMode`. If handed a both-TTY context with no mode flag it grabs the
  terminal — *and the plugin changes none of that.* The takeover is fixed by
  Axis 2 (headless launch, or a managed PTY), with the plugin riding **inside**
  the headless/managed process (extensions load in rpc/json/print too).

This is exactly why pi misbehaves where `claude -p`, headless `codex`,
`opencode`, and in-process `nex` do not: those are headless **by contract**; pi
*can* be launched into a TTY-grabbing interactive mode. Axis 2 makes pi headless
(or PTY-contained) by contract too.

---

## 2. Axis 1 — the `wg-pi-plugin` package (integration channel)

Full design in [`plugin-research.md`](plugin-research.md) §2. Summary:

- **Package:** `@worksgood/wg-pi-plugin`, keyword `pi-package`, in-repo at
  `pi-plugin/` so it version-locks to the `wg` binary it shells to. Pi-core deps
  in `peerDependencies: "*"` (provided by pi at load). Built artifact published to
  npm and/or pinned `git:` ref.
- **Layout (`pi-plugin/src/`):** `index.ts` (registration entry), `tools.ts`
  (`wg_ready`/`wg_show`/`wg_add`/`wg_done`/`wg_fail`/`wg_msg_*`/`wg_run`),
  `commands.ts` (`/wg`, `/wg-model`), `graph-widget.ts`
  (`setWidget`/`setStatus` on `session_start`/`turn_end`), `model-bridge.ts`
  (`registerProvider` + `model_select` → `CoordinatorState.model_override`),
  `wg-backend.ts` (`pi.exec("wg", …)` today; daemon-IPC client later).
- **Hooks used:** `registerTool` ×N, `registerCommand`, `registerProvider`,
  `setModel`/`model_select`, `session_start`/`turn_end`, `before_agent_start`
  (inject wg task context), `session_shutdown`, `pi.events`/`createEventBus`.
- **Config knobs ride in via env** WG already exports to handlers
  (`WG_TASK_ID`, `WG_AGENT_ID`, `WG_CHAT_ID`, `WG_STATE_DIR`, daemon socket),
  read inside the extension factory.

### 2.1 Three deployment topologies for `ExecutorKind::Pi`

| Topology | What WG spawns | Plugin delivery | Takeover | Use |
|---|---|---|---|---|
| **A — RPC + auto-loaded plugin** | `pi --mode rpc` | installed in `~/.pi/agent/extensions/` or `pi -e <plugin>` or settings `packages` | none (headless launch, Axis 2 (b)) | cheapest; smallest delta from wrapper P1a; **ship first** |
| **B — SDK Node host** | `node wg-pi-host.mjs` | in-process JS object via `DefaultResourceLoader({extensionFactories:[wgPlugin], eventBus})` | none (no terminal at all) | deterministic version, clean event-bus↔IPC bridge; **default for unattended workers/chat** |
| **C — pi hosts, human drives WG** | (nothing — human runs `pi`) | auto-discovered global plugin | desired (the product) | attended front-end; takeover is the UX |

**Recommendation (from `plugin-research.md` §4.4):** ship A first (validates the
plugin), make B the default for unattended `ExecutorKind::Pi`, document C as the
attended front-end.

### 2.2 Model management is native through the plugin

- **Warm mid-chat swap:** a `/wg-model <spec>` command (or pi's built-in `/model`)
  resolves via `ctx.modelRegistry.find(...)` and calls `pi.setModel(model)` — no
  respawn (`plugin-research.md` §1.3). This **promotes** the wrapper plan's
  "optional Layer 2 pi warm swap" (`integration-plan.md` §2.2) to the *normal*
  path.
- **Write-back:** the plugin subscribes to `model_select` and writes the choice
  into WG's `CoordinatorState.model_override` (`wg chat model …` or daemon IPC) so
  it survives a WG-side respawn (the identity bridge, `integration-plan.md` §2.3).
- **Provider bridge:** `pi.registerProvider()` injects WG's configured
  endpoints/keys (from `wg secret`/the active profile) so pi's native `/model`
  picker lists exactly WG's models.

---

## 3. Axis 2 — takeover fix (orthogonal, required for WG-spawns-pi)

This axis is **independent of the plugin** and **required** whenever WG spawns pi
unattended. Two interchangeable mechanisms (pick per situation):

### 3.1 Generic terminal-host (the fallback + other-tools path)

Full design in [`terminal-host-research.md`](terminal-host-research.md) §4. WG
owns a `HostedChild` spec + `TerminalProfile` (declarative per-tool data) + a
`TerminalHost` trait with one method per mode:

| Mode | Method | For pi | Backed by today |
|---|---|---|---|
| a — embed in TUI pane | `embed` | render pi in a WG pane via a **private PTY** (grab contained) | `PtyPane::spawn_via_tmux`/`spawn_in` |
| b — headless/detached | `run_headless` | run pi as a worker with null/file stdio (no TTY) | `execution.rs` + `write_wrapper_script` |
| c — handoff | `handoff` | drop into interactive pi, return to WG | `exec.rs` inherit + **new** suspend/restore |
| d — protocol | `open_protocol` | piped JSONL/RPC, no PTY (`pi --mode rpc`) | `opencode_handler.rs` framing |
| e — standalone PTY host | `host_fullscreen` | pi filling the window | `tui_pty.rs`/`tui_nex.rs` |

pi's `TerminalProfile`: `alt_screen=false`, `needs_capability_replies=true` (for
an embedded pi), `rpc_capable=true`, `headless_flag=["--mode","rpc"]` or `["-p"]`,
`exits_on_error_headless=true`.

**This is the fallback and the other-tools path** (the task's explicit
requirement, `terminal-host-research.md` §5.1/§5.2):

- **Fallback for pi:** when the plugin is absent/unsuitable, WG hosts pi as a
  guest — mode a/e (managed PTY, grab contained) or mode b/d (headless, no TTY).
  **WG-spawns-pi-as-a-worker is a primary, plugin-independent terminal-host use
  case.**
- **Primary mechanism for every non-plugin tool:** `claude`/`codex` CLIs,
  `aider`, `opencode`, arbitrary REPLs can only ever be *hosted as guests*; the
  terminal-host is their sole integration route. Adding a tool becomes data entry
  (a `TerminalProfile`), not a new `build_*_chat_pty_args` / `*_handler.rs`.

The genuinely-new piece is the **transient handoff take-back** (mode c):
`OuterTerminal::suspend/resume` extracted from `viz_viewer/mod.rs:162-167`
(`disable_raw_mode`/`LeaveAlternateScreen`/`DisableBracketedPaste`), `handoff`
running the child on the real terminal with `Stdio::inherit`, then restoring the
TUI. The **capability-query responder** becomes a host guarantee keyed on
`profile.needs_capability_replies` (`terminal-host-research.md` §4.3/§4.4).

> **Scoping note:** the full generic terminal-host port-out (port-embed,
> port-headless, the standalone full handoff) is owned by
> `terminal-host-research.md` §6 as its **own** track and should be bootstrapped
> there — not duplicated into the pi graph. This plan creates only the
> terminal-host **trait foundation** and the **pi consumer/takeover-fix** that sit
> on the pi critical path (see §6).

### 3.2 Pi headless mode (the alternative takeover fix)

The other interchangeable mechanism: launch pi headless so it never grabs the
terminal — `--mode rpc` (live chat) or `-p`/`--mode json` (one-shot worker),
and/or piped/`null` stdio (any one fd not a TTY defeats the grab). The plugin
rides *inside* that headless process. Optionally the **staged `PI_NO_TUI`**
default-off env switch (`integration-plan.md` §3, prepared under
`upstream-patch/` by `pi-impl-p5`) forces non-interactive even under a both-TTY
PTY — a belt-and-suspenders kill-switch for a launcher that cannot guarantee the
flags.

**Reframe of `PI_NO_TUI` (revised user guidance):** the wrapper plan and
`plugin-research.md` §3 called `PI_NO_TUI` "moot." That verdict applied only to
the *direction-split* reasoning. Per the corrected analysis, `PI_NO_TUI`/pi
headless is a **real takeover fix for the executor path** and is **kept** as one
of the two orthogonal Axis-2 mechanisms (not abandoned). It remains
*optional/belt-and-suspenders* relative to the headless-launch contract, and the
staged PR stays **prepared but unsubmitted** (the human, Shape-B/Topology-C-scoped
`.flip` decision).

---

## 4. WG-side wiring (kept/adapted from the wrapper plan)

- **`ExecutorKind::Pi`** — **kept** (pi-impl-p0, done): in `EXTERNAL_CLIS`, not
  `WORKER_ONLY_EXTERNALS`; routing free through `handler_for_model`.
- **Discovery — extended:** a `pi:` route is satisfiable by **either** a `pi`
  binary (Topology A/C) **or** Node + `wg-pi-host` + the plugin bundle (Topology
  B). `src/executor_discovery.rs`.
- **`wg pi-handler`** — **adapted:** instead of prompt-munging, it (A) spawns
  `pi --mode rpc` with the plugin present and bridges the plugin event bus ↔ WG
  IPC, or (B) spawns `node wg-pi-host.mjs`. The headless transport (mode b/d)
  still defeats the takeover in direction (2). `src/commands/pi_handler.rs`,
  `src/cli.rs`, `src/main.rs`.
- **`pi.toml` profile — kept, gains plugin install:** pi routes + `claude:haiku`
  agency roles (unchanged), **plus** documenting plugin placement
  (`~/.pi/agent/extensions/` global vs `.pi/extensions` project trust gate) and,
  for Topology B, the Node-host bundle. `src/profile/templates/pi.toml`.
- **`wg chat model <id> <spec>` — kept, fuses with the plugin:** ergonomic alias
  over the existing `SetChatExecutor` IPC; for a live pi/plugin session the swap
  goes through pi-native `setModel` (warm) instead of a respawn. `src/cli.rs`,
  `src/commands/chat_cmd.rs`.
- **TUI `/model` picker — kept for WG-native panes; replaced for the pi
  front-end:** WG's RPC-rendered transcript (Topology A/B) still needs its own
  picker; in pi's interactive TUI (Topology C) the human uses pi's **native**
  `/model` and the plugin round-trips the choice. `src/tui/viz_viewer/chat_*`.
- **Warm swap — split:** pi half **subsumed into the plugin** (§2.2, now default);
  **nex** per-turn re-resolve + skip-SIGTERM-same-kind kept as its own optional
  task. `src/nex_runtime.rs`, `src/commands/service/ipc.rs`.
- **Smoke + credentialed validation — kept, expanded:** add plugin scenarios
  (loads in rpc/tui/print; wg tools callable; `/wg` works; `model_select`
  write-back; graph widget renders) and **keep the takeover-regression guard**
  (headless pi never enters raw mode — guards direction 2, which the plugin does
  *not* fix). `tests/smoke/scenarios/`, `tests/smoke/manifest.toml`.

---

## 5. Reconciliation of the wrapper P0–P5 tasks (KEEP / ADAPT / ABANDON)

The existing `pi-impl-*` tasks were created by `pi-impl-bootstrap` from
`integration-plan.md` §5. p0 and p5 are **done**; p1a/p1b/p2a/p2b/p3/p4a/p4b are
**PAUSED**. Verdicts (executed: PAUSED→ABANDON with `--superseded-by`; done→kept
in place):

| Old task | Status | Verdict | Action | Superseded by |
|---|---|---|---|---|
| `pi-impl-p0-executor-kind` | done | **KEEP** | none (done; `ExecutorKind::Pi` still needed). Discovery *extension* for the Node-host path lands in the new handler task. | — |
| `pi-impl-p1a-handler` | PAUSED | **ADAPT** | abandon (wrapper prompt-munging framing) | `pi-plugin-impl-handler` |
| `pi-impl-p1b-profile` | PAUSED | **ADAPT** (≈KEEP, gains plugin-install) | abandon | `pi-plugin-impl-profile` |
| `pi-impl-p2a-chat-model-verb` | PAUSED | **ADAPT** (gains warm pi path via plugin) | abandon | `pi-plugin-impl-chat-model-verb` |
| `pi-impl-p2b-tui-model-picker` | PAUSED | **ADAPT** (KEEP for WG panes; replaced for pi front-end) | abandon | `pi-plugin-impl-tui-model-picker` |
| `pi-impl-p3-warm-swap` | PAUSED | **ADAPT/SPLIT** (pi half → plugin; nex half kept) | abandon | `pi-plugin-impl-nex-warm-swap` + `pi-plugin-impl-package` |
| `pi-impl-p4a-smoke` | PAUSED | **ADAPT/EXPAND** (+ plugin scenarios) | abandon | `pi-plugin-impl-smoke` |
| `pi-impl-p4b-live-validation` | PAUSED | **ADAPT/EXPAND** (+ plugin-in-loop) | abandon | `pi-plugin-impl-live-validation` |
| `pi-impl-p5-upstream-patch` | done | **KEEP — reframed** (real Axis-2 takeover-fix option, not "moot") | none (done; staged PR stays unsubmitted). Referenced by `pi-plugin-impl-pi-takeover-fix`. | — |

Net: the plugin pivot **keeps** WG-side routing + the done foundation
(p0, p5 staged artifact), **adapts** every PAUSED wrapper task into a
plugin-routed equivalent, **splits** P3 (pi→plugin, nex→own task), and **adds**
the plugin package, the SDK host, and the generic terminal-host trait + pi
consumer.

---

## 6. New plugin-first implementation task graph

10 new `pi-plugin-impl-*` tasks, gated behind this replan (`.flip-pi-plugin-replan`).
Same-file sequencing enforced (golden rule: same file ⇒ sequential edge).

**Shared-file ledger:**
- `pi-plugin/**` — only `pi-plugin-impl-package`.
- `src/commands/pi_handler.rs` — `pi-plugin-impl-handler`, then
  `pi-plugin-impl-pi-takeover-fix` (sequential).
- `src/cli.rs` — `pi-plugin-impl-handler`, then `pi-plugin-impl-chat-model-verb`
  (sequential).
- `src/terminal_host/` — `pi-plugin-impl-terminal-host-trait`, then
  `pi-plugin-impl-pi-takeover-fix` (sequential).
- `src/nex_runtime.rs` + `src/commands/service/ipc.rs` — only
  `pi-plugin-impl-nex-warm-swap`.
- `src/tui/viz_viewer/chat_*` — only `pi-plugin-impl-tui-model-picker`.
- `tests/smoke/manifest.toml` — `pi-plugin-impl-smoke` owns all pi entries;
  `pi-plugin-impl-live-validation` is `--after` it (sequential manifest touch).

**Graph:**
```
pi-plugin-replan (this replan, human-reviewed via .flip)
 ├─ pi-plugin-impl-package ───────────────┐
 │   (pi-plugin/ TS pkg + wg-pi-host.mjs;  │
 │    model-bridge subsumes P3 pi half)    │
 │                                         ▼
 │                              pi-plugin-impl-handler ──┬─ pi-plugin-impl-chat-model-verb ─┬─ pi-plugin-impl-tui-model-picker
 │   (after package + p0/done)  (pi_handler/cli/main +   │   (cli.rs shared → after handler) ├─ pi-plugin-impl-nex-warm-swap
 │                               executor_discovery;     │                                   │   (nex half; own files)
 │                               Topology A & B)         │
 ├─ pi-plugin-impl-terminal-host-trait ─┐               ├─ pi-plugin-impl-smoke ── pi-plugin-impl-live-validation
 │   (src/terminal_host/ trait; the      │               │   (after handler + takeover-fix)
 │    fallback + other-tools foundation)  ▼               │
 │                            pi-plugin-impl-pi-takeover-fix
 │   (after trait + handler; pi TerminalProfile via modes b/d & a/e;
 │    references PI_NO_TUI/pi-impl-p5 as the alt mechanism)
 └─ pi-plugin-impl-profile  (after p0/done; pi.toml + plugin install; own files)
```

Each task is "implement directly — do not decompose further", carries a
`## Validation` section, and (where user-visible: the TUI picker, the takeover
guard, the smoke scenarios) requires a live PTY/tmux human-flow reproducer per
CLAUDE.md, not a CLI substitute.

**Out of scope here (owned elsewhere):** the full generic terminal-host
port-out (`terminal-host-port-embed`, `terminal-host-handoff`,
`terminal-host-port-headless`) is `terminal-host-research.md` §6's own track —
bootstrapped separately to avoid duplicating that graph. This plan creates only
the trait foundation + the pi consumer on the pi critical path.

---

## 7. Open questions for the human reviewer

1. **Topology default.** Ship A first then default unattended workers to B
   (recommended), or commit to B from the start (deterministic but adds the Node
   host dependency immediately)?
2. **Node dependency depth.** Topology B makes Node + the plugin bundle a hard
   prerequisite for `ExecutorKind::Pi`. Acceptable for an *experimental* handler;
   confirm it stays out of any default rebase (§0.6).
3. **`PI_NO_TUI` submission.** The staged PR (`pi-impl-p5`) stays unsubmitted by
   default; open it only if the attended-TUI (Topology C) path with a
   flag-blind launcher is pursued.
4. **Plugin distribution.** npm publish vs pinned `git:` ref vs in-repo build +
   `~/.pi/agent/extensions/` placement — and the project-trust gate for
   `.pi/extensions` (`plugin-research.md` §6.3).
5. **Pi version pin.** Pi-core APIs are `peerDependencies: "*"` and unversioned;
   the smoke task should fail if `ExtensionAPI`/`setModel`/`registerProvider`
   signatures drift (tested against 0.79.x).
6. **Promotion gate.** pi stays in `EXPERIMENTAL_EXTERNAL_EXECUTORS` until
   `pi-plugin-impl-live-validation` is green (credentialed RPC streaming, resume,
   large output, plugin-in-the-loop).

---

## Sources

- [`docs/pi-integration/plugin-research.md`](plugin-research.md) — pi extension
  API verdict, `wg-pi-plugin` package, three topologies, replaces-vs-keeps §5,
  two-direction takeover analysis §3.
- [`docs/pi-integration/terminal-host-research.md`](terminal-host-research.md) —
  generic `TerminalHost`/`TerminalProfile` layer (modes a–e), plugin-orthogonality
  §5, follow-up tasks §6.
- [`docs/pi-integration/integration-plan.md`](integration-plan.md) — the prior
  wrapper-first plan (P0–P5) this v2 supersedes for the in-session surface.
- [`docs/pi-integration/executor-research.md`](executor-research.md),
  [`docs/pi-integration/model-mgmt-research.md`](model-mgmt-research.md) — pi
  takeover root cause, warm `set_model`, identity bridge.
- [`docs/pi-integration/upstream-patch/`](upstream-patch/README.md) — the staged,
  unsubmitted `PI_NO_TUI` PR (`pi-impl-p5`, kept as an Axis-2 option).
- Revised user guidance on `pi-impl-p5` (queued to `pi-plugin-replan`,
  2026-06-22): plugin = integration channel and terminal-host-or-headless =
  takeover fix are orthogonal, both required.
- WG code anchors enumerated in the header (verified this branch).
</content>
</invoke>
