# Pi Plugin/Extension API + `pi-worksgood` Package Design

**Task:** `pi-plugin-research` · **Date:** 2026-06-22
**Status:** **Investigation only — no production code changed.**

This document determines whether **pi** (the Pi Coding Agent, npm
`@earendil-works/pi-coding-agent`) exposes a plugin/extension API, and designs a
first-class WG integration **package** that pi loads. It supersedes the
external-CLI + `PI_NO_TUI` *wrapper* direction in
[`executor-research.md`](executor-research.md) and
[`integration-plan.md`](integration-plan.md) for the *in-session* surface, while
**keeping** the wrapper's headless-launch contract for one specific reason made
precise in §3.

Source examined: `@earendil-works/pi-coding-agent@0.79.4` (unpacked from
`npm pack`), TUI sub-package `@earendil-works/pi-tui@0.79.9`. All citations below
are to that tarball's `docs/`, `examples/`, and `dist/`.

---

## 0. Verdict (the headline)

> **Pi has a first-class, documented, load-bearing extension/plugin API
> *today*. No additive upstream hook is required to build a WG integration that
> pi loads. The previously-proposed `PI_NO_TUI` upstream patch (wrapper task P5)
> becomes moot — but for a precise reason that is *not* "the plugin fixes the
> takeover" (see §3).**

Evidence (all in the shipped 0.79.4 package):

| Artifact | What it proves |
|---|---|
| `docs/extensions.md` (102 KB) | A complete, stable extension contract: lifecycle events, custom tools, custom commands, custom providers, shortcuts, flags, UI widgets, state persistence, custom rendering. |
| `docs/sdk.md` (34 KB) | A programmatic embedding API (`createAgentSession`, `DefaultResourceLoader`, `createEventBus`) — pi can be embedded *as a library* with extensions injected as in-process JS objects. |
| `examples/extensions/` (**77 files**) | Mature, exercised extension surface — `tools.ts`, `dynamic-tools.ts`, `handoff.ts`, `event-bus.ts`, `model-status.ts`, `permission-gate.ts`, `custom-header.ts`/`custom-footer.ts`, `structured-output.ts`, `ssh.ts`, … |
| `dist/index.d.ts` exports | Public, typed API: `ExtensionAPI`, `ExtensionContext`, `ExtensionCommandContext`, `DefaultResourceLoader`, `createAgentSession`, `createAgentSessionRuntime`, `createEventBus`, `ModelRegistry`, `AuthStorage`, `SessionManager`, `defineTool`, `isToolCallEventType`. |
| `dist/main.js:363,510,521` | Pi's **own** interactive/rpc/print/json modes are built on `DefaultResourceLoader` + `extensionFactories`/`additionalExtensionPaths` — extensions are not a bolt-on, they are how pi loads its own runtime. |

So this is **not** the "no plugin API → propose a minimal additive hook" branch.
It is the "rich plugin API exists → design the WG package against it" branch.

### What the plugin API gives us (the surface that matters for WG)

From `docs/extensions.md`:

- **Custom tools the LLM can call** — `pi.registerTool({name, parameters, execute})`
  (`extensions.md:1269`, example `examples/extensions/tools.ts`). Tools can be
  added/removed at runtime and toggled with `pi.setActiveTools()`
  (`extensions.md:1273`, `:1535`). → *run wg tasks from inside pi.*
- **Custom slash commands** — `pi.registerCommand("wg", {handler, getArgumentCompletions})`
  (`extensions.md:1424`), with argument autocomplete. → `/wg ready`, `/wg run …`.
- **Programmatic model switch** — `pi.setModel(model)` (`extensions.md:1562`),
  `pi.setThinkingLevel(level)` (`extensions.md:1576`), `model_select` event
  (`extensions.md:660`), and on the SDK session `cycleModel()`/`cycleThinkingLevel()`
  (`sdk.md:91-94`). `ctx.modelRegistry.find(provider, id)` resolves a model
  (`extensions.md:919`). → *switch models mid-chat via pi's native model manager.*
- **Custom providers/endpoints** — `pi.registerProvider(name, config)`
  (`extensions.md:1594`), sync or via an async factory that can fetch a remote
  model list before startup (`extensions.md:189-217`). → bridge WG's model
  registry/profiles into pi.
- **In-session UI surfaces** — `ctx.ui.setWidget(id, lines)` (widget above the
  editor), `ctx.ui.setStatus(id, text)` (footer), `ctx.ui.notify()`,
  `ctx.ui.custom()` for full TUI components (`extensions.md:166-167`, `:885`,
  `tools.ts:78`). → *surface the wg task graph in-session.*
- **Full lifecycle hooks** (`extensions.md:272-872`): `session_start`,
  `before_agent_start` (inject context / rewrite system prompt),
  `tool_call` (**block/mutate** a tool call), `tool_result` (modify),
  `turn_start`/`turn_end`, `agent_end`, `model_select`, `session_shutdown`, `input`
  (intercept/transform/handle user input), etc.
- **Message injection & shell-out** — `pi.sendUserMessage()` / `pi.sendMessage()`
  (`extensions.md:1320-1369`), `pi.exec(cmd, args, {signal})` (`extensions.md:1526`).
- **State + cross-extension bus** — `pi.appendEntry()` session-persisted state
  (`extensions.md:1371`), `pi.events.on/emit` shared event bus (`extensions.md:1585`),
  also reachable from an SDK host via `createEventBus()` (`sdk.md:581-592`).
- **Mode-independent loading** — extensions run in **all four** pi modes
  (`extensions.md` "Mode Behavior"): `tui` / `rpc` (both `ctx.hasUI === true`),
  `json` / `print` (`hasUI === false`, UI no-ops, but tools/events still fire).
  This is the property the whole design leans on.

### Two distribution mechanisms (both first-class)

1. **As a loaded extension** — drop `.ts` under `~/.pi/agent/extensions/` (global)
   or `.pi/extensions/` (project), or `pi -e ./path.ts`, or `pi install npm:…`/
   `git:…` as a **pi package** (`packages.md`). Loaded by jiti, no build step
   (`extensions.md:108-136`, `:178`).
2. **As an SDK-embedded factory** — a Node host calls
   `createAgentSession({ resourceLoader: new DefaultResourceLoader({ extensionFactories: [wgPlugin], eventBus }) })`
   and passes the plugin **as an in-process JS function object** plus a shared
   `eventBus` it can also listen on (`sdk.md:559-595`, `:545-553`). No file
   discovery, deterministic version. This is the primitive that lets **WG own the
   embedding**.

---

## 1. Plugin-first: how the package answers each task question

The pivot's thesis is *invert the relationship*. In the wrapper model WG drives
pi as a subordinate CLI (spawn `pi -p`, munge a prompt, scrape stdout). In the
plugin model the WG↔pi integration lives **inside a pi session** as a loaded
module — the same artifact whether a human launched pi or WG did.

### 1.1 Run wg tasks from inside pi → tools + commands

Register a small tool family the LLM (and the human via `/wg`) can call. Each
tool shells out to the `wg` binary with `pi.exec("wg", [...])` (or, later, talks
to the WG daemon IPC socket directly — see §4.4):

```ts
pi.registerTool({
  name: "wg_ready",
  label: "WG: ready tasks",
  description: "List WG tasks ready to be worked on.",
  parameters: Type.Object({}),
  async execute(_id, _p, signal) {
    const r = await pi.exec("wg", ["ready", "--json"], { signal });
    return { content: [{ type: "text", text: r.stdout }], details: JSON.parse(r.stdout || "[]") };
  },
});
// plus: wg_show, wg_add, wg_log, wg_done, wg_fail, wg_msg_send … (one per verb, or one
// dispatch tool with an action enum). Mirrors examples/extensions/tools.ts.

pi.registerCommand("wg", {
  description: "WG task graph: /wg ready | /wg run <id> | /wg graph | /wg show <id>",
  getArgumentCompletions: (prefix) => /* ready|run|graph|show|add … */ null,
  handler: async (args, ctx) => { /* dispatch to pi.exec("wg", …) + ctx.ui */ },
});
```

"Run a wg task from inside pi" = `/wg run <id>` → the command shells `wg show`/
assembles the prompt and `pi.sendUserMessage(prompt)` (or drives a sub-session)
so pi's own agent loop executes the task with full wg tooling in context. Because
tools are callable by the LLM, an attended human can also just *ask* pi to "pick
up the next ready wg task" and it will call `wg_ready` → `wg_show` → work.

### 1.2 Explicit wg access only; no passive ready-task UI

The plugin exposes WG through tools and slash commands. It must not install an
always-visible ready-task widget/status footer on `session_start` or `turn_end`:
that duplicates the WG TUI and pollutes Pi chat sessions with state the user can
already inspect directly.

If WG later wants embedded graph UI, it should be an explicit `/wg graph` or
user-triggered command surface, not automatic session chrome.

### 1.3 Switch models mid-chat via pi's native model manager → `setModel` + round-trip

Pi already does warm, in-process model switching (`model-mgmt-research.md` §2):
TUI `/model` picker, `Ctrl+P` cycle, RPC `set_model`/`cycle_model`. The plugin
plugs into this *natively*:

- A `/wg-model <spec>` command (or letting the human use pi's built-in `/model`)
  resolves via `ctx.modelRegistry.find(...)` and calls `pi.setModel(model)`
  (`extensions.md:1562`). No respawn — pi mutates the live session's model pointer
  and the next turn uses it (`model-mgmt-research.md` §2.3).
- The plugin subscribes to `model_select` (`extensions.md:660`,
  `examples/extensions/model-status.ts`) and **writes the choice back into WG's
  `CoordinatorState.model_override`** (via `wg chat model …` or daemon IPC) so the
  selection survives a WG-side respawn and the two model views stay coherent
  (the identity bridge from `model-mgmt-research.md` §6.2).
- `pi.registerProvider()` (`extensions.md:1594`) lets the plugin inject WG's
  configured endpoints/keys (from `wg secret` / the active profile) into pi's
  model registry, so pi's native `/model` picker lists exactly WG's models.

This is the cleanest realization of the "warm `/model` swap for a CLI-class
handler" that `integration-plan.md` §2.2 flagged as pi's distinctive value — and
here it is *native*, not a WG-built affordance.

### 1.4 Terminal-takeover — **the plugin solves it in ONE direction only**

This is the subtlety (and a scope correction applied during this task). **The
plugin does not change pi's invocation or mode.** It is code pi loads *after*
`resolveAppMode` has already decided whether to grab the terminal. Loading a
plugin neither causes nor prevents `setRawMode(true)`. So the two directions must
be analyzed separately — see §3, which is the authoritative treatment.

---

## 2. The `pi-worksgood` package

### 2.1 Name & shape

- **Package name:** `@worksgood/pi` (npm-publishable pi package) with the
  `pi-package` keyword (`packages.md:116`). Bin-less; it is a resource bundle, not
  a CLI.
- **In-repo home:** `worksgood-pi/` at the WG repo root (TS sources), built/bundled
  into the package. Keeping it **in-repo** (not a separate repo) is recommended so
  it version-locks to the WG binary it shells to and ships in the same release.
  Distribute the built artifact to npm (and/or a pinned `git:` ref) so
  `pi install` / settings `packages` can pull it.
- **Manifest** (`package.json`):

  ```json
  {
    "name": "@worksgood/pi",
    "version": "0.1.0",
    "keywords": ["pi-package"],
    "type": "module",
    "main": "./pi-worksgood/index.js",
    "pi": { "extensions": ["./pi-worksgood/index.js"] },
    "peerDependencies": {
      "@earendil-works/pi-coding-agent": "*",
      "@earendil-works/pi-tui": "*",
      "typebox": "*"
    }
  }
  ```

  Pi-core packages go in `peerDependencies` with `"*"` and are **not** bundled
  (`packages.md:169`) — pi provides them at load time.

### 2.2 Layout

```
worksgood-pi/
├── package.json            # pi-package manifest (above)
├── src/
│   ├── index.ts            # export default function (pi: ExtensionAPI) — registration entry
│   ├── tools.ts            # wg_ready / wg_show / wg_add / wg_done / wg_fail / wg_msg_* / wg_run
│   ├── commands.ts         # /wg, /wg-model
│   ├── graph-widget.ts     # legacy no-op compatibility exports
│   ├── model-bridge.ts     # registerProvider(wg endpoints) + model_select → CoordinatorState
│   └── wg-backend.ts       # exec("wg", …) today; daemon-IPC client later (§4.4)
└── dist/                   # built output referenced by the manifest
```

### 2.3 Registration (`src/index.ts`)

```ts
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { registerWgTools } from "./tools.js";
import { registerWgCommands } from "./commands.js";
import { installModelBridge } from "./model-bridge.js";

export default function wgPlugin(pi: ExtensionAPI) {
  registerWgTools(pi);       // pi.registerTool(...) × N
  registerWgCommands(pi);    // pi.registerCommand("wg", ...), "wg-model"
  installModelBridge(pi);    // pi.registerProvider(...); pi.on("model_select", writeBackToWG)
}
```

Config knobs (which WG chat/task this session is bound to, daemon socket path,
whether to inject the task graph) ride in via env (`WG_TASK_ID`, `WG_AGENT_ID`,
`WG_CHAT_ID`, `WG_STATE_DIR`) read inside the factory — exactly the env WG already
exports to handlers (`integration-plan.md` §1.3). Defer any long-lived resource
(socket, watcher) to explicit connection setup and tear down in `session_shutdown`
(`extensions.md:219-223`).

### 2.4 Hooks summary (what the plugin subscribes to)

| Hook / API | Used for |
|---|---|
| `registerTool` × N | wg verbs callable by the LLM / human (run tasks from pi) |
| `registerCommand("wg"/"wg-model")` | human slash-command surface + autocomplete |
| `registerProvider` | inject WG endpoints/keys into pi's model registry |
| `setModel` / `model_select` | native warm mid-chat switch + write-back to `CoordinatorState` |
| `before_agent_start` | inject current wg task context / system-prompt addendum |
| `agent_end` | optional: `wg log` a breadcrumb of the turn |
| `session_shutdown` | close daemon socket / watchers |
| `pi.events` / `createEventBus` | bridge to a WG SDK host (§4.2) |

---

## 3. Terminal-takeover, analyzed in BOTH directions

The takeover root cause is unchanged from `executor-research.md` §1: pi enters
the full-screen TUI **iff** `resolveAppMode` returns `interactive`, which happens
**only** when stdin **and** stdout are both TTYs and no `--mode`/`-p` flag is set
(`dist/main.js:77-88`); pi-tui then calls `setRawMode(true)` (`terminal.js:80`)
and claims the terminal. **The plugin is loaded *after* this decision and cannot
influence it.** Two directions follow.

### Direction (1) — pi-as-host + wg plugin: takeover is **moot** (the product)

A **human** runs `pi` interactively (or attaches it to a WG chat via
`--session-id`). The WorksGood Pi extension is auto-discovered from
`~/.pi/agent/extensions/` (or installed as a pi package). Here the full-screen
takeover is exactly what the user wants — pi's native TUI, `/model` picker,
steering, fork/clone, plus the plugin's `/wg` commands, wg tools, and task-graph
widget. WG does **not** spawn this process; pi owns the terminal because the human
asked it to. **The plugin fully delivers the integration here, and "takeover" is
not a problem to solve — it is the UX.** This is the direction the plugin makes
genuinely new: a human can drive the entire WG graph from inside pi.

### Direction (2) — WG-spawns-pi as worker/executor: takeover **persists** (plugin does NOT fix it)

When **WG** spawns pi as an unattended worker/chat handler, pi still runs
`resolveAppMode`. If WG hands it a PTY on both ends with no mode flag (e.g. via
the generic `src/tui/pty_pane.rs` portable-pty embed), pi grabs the terminal —
**and loading the wg plugin changes none of that**, because the plugin is just
code, not an invocation/mode change. This is precisely why pi misbehaves where
`claude -p`, headless `codex`, `opencode`, and in-process `nex` do not: those are
launched headless by contract; pi *can* be launched into a TTY-grabbing
interactive mode.

**The fix for direction (2) is the same as the wrapper research already
prescribed, and it is independent of the plugin:**

- Launch pi **headless** — `--mode rpc` (live chat) or `-p` / `--mode json`
  (one-shot worker), and/or piped/`null` stdio so at least one fd is not a TTY
  (`executor-research.md` §1.4, §3, §4.1). Either condition alone defeats the
  grab. The plugin then **rides inside that headless process** (extensions load
  in `rpc`/`json`/`print` too — `extensions.md` "Mode Behavior") to provide the
  wg tools, warm `set_model`, and task-graph events — but it is the **headless
  launch, not the plugin, that prevents the takeover.**
- *If* WG deliberately wants to render pi's **real interactive TUI inside a WG
  pane**, it uses the generic terminal-host PTY embed (`pty_pane.rs`) — and there
  the takeover is **expected and correct** (pi is supposed to drive that PTY,
  like `wg nex`/octomind/dexto already do). That is a both-TTY case by design, not
  a bug.

### Consequence for the `PI_NO_TUI` upstream patch (wrapper P5)

`PI_NO_TUI` was a belt-and-suspenders kill-switch for the *hypothetical*
"WG embeds pi's real TUI through a both-TTY PTY **and** wants to force it
non-interactive from outside" (`executor-research.md` §4.2,
`integration-plan.md` §3). The plugin pivot dissolves that hypothetical by
cleanly **splitting the two directions**:

- Direction (1): the interactive TUI is the **human's** front-end — takeover is
  desired, no external kill-switch wanted.
- Direction (2): WG launches **headless** (no TTY) — takeover never occurs, no
  patch needed; or WG uses the generic PTY host where takeover is **intended**.

So `PI_NO_TUI` (P5) remains **optional/moot** — but note the precise reason: it is
moot because of the **headless-launch contract for direction (2)** (unchanged
from the wrapper research) *plus* the direction-split, **not** because "the plugin
runs inside the lifecycle." The plugin does not defeat the grab.

> **One-line summary:** The plugin removes the terminal-takeover *concern* only in
> direction (1) (human hosts pi; takeover is the product). In direction (2) (WG
> spawns pi) the takeover persists and is defeated exactly as before — by headless
> invocation — with the plugin riding *inside* the headless process. No upstream
> pi change is required for either.

---

## 4. How `ExecutorKind::Pi` routes THROUGH the plugin

WG is a Rust binary and cannot host a TS extension in its own process, so
"routes through the plugin" means *WG launches a pi runtime that has the plugin
loaded, and all in-session WG-awareness comes from the plugin* (not from WG
prompt-munging/scraping). One plugin artifact, three deployment topologies:

### 4.1 Topology A — RPC handler + auto-loaded plugin (cheapest; reuses wrapper P1a)

WG's `ExecutorKind::Pi` spawns `pi --mode rpc` with the plugin present (installed
in `~/.pi/agent/extensions/`, or `pi -e <plugin>`, or settings `packages`). The
existing RPC poll-loop handler (wrapper P1a) is the transport; the plugin supplies
wg tools/commands/widget/`set_model` **inside** the session. Headless launch ⇒ no
takeover (direction 2). Smallest delta from the wrapper plan.

### 4.2 Topology B — SDK Node host embedding the plugin (strongest "plugin-first")

WG ships a tiny Node host (`wg-pi-host.mjs`) that does:

```ts
import { createAgentSession, DefaultResourceLoader, createEventBus,
         ModelRegistry, AuthStorage, SessionManager } from "@earendil-works/pi-coding-agent";
import wgPlugin from "@worksgood/pi";

const eventBus = createEventBus();
const loader = new DefaultResourceLoader({ extensionFactories: [wgPlugin], eventBus });
const auth = AuthStorage.create();
const { session } = await createAgentSession({
  authStorage: auth, modelRegistry: ModelRegistry.create(auth),
  sessionManager: SessionManager.fromFile(process.env.WG_SESSION_FILE), resourceLoader: loader,
});
eventBus.on("wg:turn", (d) => /* forward to WG over stdio/IPC */);
```

`ExecutorKind::Pi` spawns `node wg-pi-host.mjs` instead of `pi`. The plugin is an
**in-process JS object** (deterministic version, no user config), and the shared
`eventBus` bridges plugin events ↔ WG IPC directly — no terminal involved at all
(direction 2, headless by construction). This is "pi as a library," and is the
truest realization of "ALL wg-pi work runs through pi via the plugin." Cost: WG
must locate Node + ship the host + plugin bundle (`executor_discovery` extension).

### 4.3 Topology C — inverted: pi hosts, human drives WG from inside (direction 1)

Human runs `pi` (interactive). Plugin auto-discovered. `/wg run <id>`, wg tools,
and pi-native `/model` with write-back. WG spawns nothing; the plugin reaches the
WG backend via `pi.exec("wg", …)` or the daemon socket. This is the attended
front-end; takeover is the product.

### 4.4 WG-side wiring (mostly unchanged from `integration-plan.md` §1.1)

- Add `ExecutorKind::Pi` to `EXTERNAL_CLIS` (not `WORKER_ONLY_EXTERNALS`); routing
  for `pi:<spec>` comes free through `handler_for_model`'s external-CLI
  interception (wrapper P0 — **kept**).
- Extend `executor_discovery` so a `pi:` route is satisfiable by **either** a `pi`
  binary (Topology A/C) **or** Node + the `wg-pi-host` + plugin bundle (Topology B).
- The plugin's `wg-backend.ts` starts as `pi.exec("wg", …)` (works everywhere) and
  can later upgrade to a daemon-IPC client for lower latency and richer events;
  the tool/command surface is identical either way.

**Recommendation:** ship Topology A first (minimal delta, validates the plugin),
make Topology B the default for unattended `ExecutorKind::Pi` workers/chat
handlers (deterministic + clean event bridge), and offer Topology C as the
documented attended front-end.

---

## 5. Replaces-vs-keeps vs the wrapper P0–P5 tasks

Wrapper tasks are from `integration-plan.md` §5 (the phased breakdown gated behind
`pi-design-integration`). None of these were implemented as production code on this
branch except the staged P5 patch under `docs/pi-integration/upstream-patch/`.

| Wrapper task | Verdict | Rationale |
|---|---|---|
| **P0 `pi-executor-kind`** (ExecutorKind::Pi + discovery) | **KEEP** (extend) | WG still must recognize `pi:` routes. Extend discovery to accept the Node-host+plugin path (Topology B), not just a `pi` binary. |
| **P1a `pi-handler`** (RPC chat + one-shot worker) | **KEEP, reframed** | Still the headless transport that defeats the takeover in direction (2). What changes: in-session wg-awareness moves *out* of WG prompt-munging and *into* the plugin. Add "ensure plugin is loaded" + "bridge plugin event bus ↔ WG IPC." For Topology B, this becomes the `node wg-pi-host` spawn rather than a `pi` spawn. |
| **P1b `pi-profile`** (`pi.toml` starter) | **KEEP** | Still need a profile pinning Pi worker/chat routes, now with explicit cheaper agency/meta overrides. |
| **P2a `wg chat model <id> <spec>`** | **KEEP**, gains warm path | Still WG's CLI verb over `SetChatExecutor`. For a live pi/plugin session, the model swap now goes through pi-native `setModel` (warm) instead of a respawn — i.e. P2a's pi branch fuses with P3's pi half. |
| **P2b `tui-model-picker`** (WG's own TUI `/model`) | **KEEP for WG-native panes; REPLACED for the pi front-end** | When WG renders the transcript (Topology A/B RPC), it still needs its own picker. When the human is in pi's interactive TUI (Topology C), they use pi's **native** `/model` and the plugin round-trips the choice — WG builds nothing for that surface. |
| **P3 `warm-swap`** (nex per-turn + pi `set_model`) | **pi half SUBSUMED into the plugin (now default, not "optional later"); nex half KEEP** | The plugin makes pi's warm `set_model` the *normal* path, not a deferred Layer-2 optimization. The nex per-turn re-resolve is pi-independent and unchanged. |
| **P4a `pi-smoke`** + **P4b `pi-live-validation`** | **KEEP, EXPAND** | Still need config-lint + credentialed validation. **Add** plugin scenarios: plugin loads in `rpc`/`tui`/`print`; wg tools callable; `/wg` command works; `model_select` writes back to `CoordinatorState`; passive ready-task widgets are not installed; **takeover-regression guard stays** (asserts headless pi never enters raw mode — this guards direction 2, which the plugin does *not* fix). |
| **P5 `pi-upstream-patch`** (`PI_NO_TUI`) | **DROP / moot** | Superseded by the direction-split (§3). No upstream pi change is required. `docs/pi-integration/upstream-patch/` stays as a documented, unsubmitted artifact; do not open the PR. |

**New work the plugin pivot introduces (not in P0–P5):**

- **`pi-worksgood` package** — the `worksgood-pi/` TS sources + npm/git packaging
  (§2). This is the center of gravity of the new direction.
- **`wg-pi-host.mjs`** — the SDK embedding host for Topology B (§4.2).
- **Plugin↔WG backend client** — `pi.exec("wg")` now, daemon-IPC later (§4.4).

Net: the plugin pivot **keeps the headless transport and WG-side routing**
(P0/P1a/P1b/P2a, the nex half of P3, P4), **replaces the in-session integration
mechanism** (wg-awareness via plugin, warm `set_model` native), **moots the
upstream patch** (P5), and **adds** the plugin package + SDK host.

---

## 6. Risks / open questions (for `pi-plugin-replan`)

1. **Node runtime dependency deepens.** Topology B makes Node + the plugin bundle
   a hard prerequisite for `ExecutorKind::Pi`. Same dependency-inversion caveat as
   `integration-plan.md` §4.2 — acceptable for an *additive* experimental handler,
   not for a default rebase.
2. **Plugin↔backend identity.** `pi.exec("wg")` inherits cwd/env; the plugin must
   bind to the right WG project/daemon (`--dir`, socket path) — wire the WG env
   (`WG_STATE_DIR`, daemon socket) into the factory and prefer explicit `--dir`
   over cwd inference (cf. the global-daemon-hazard memory).
3. **Trust gate.** Project-local `.pi/extensions` load only after pi trusts the
   project (`extensions.md:112`, `project_trust` event). A WG-installed *global*
   plugin (`~/.pi/agent/extensions/`) sidesteps this; document the placement.
4. **Version coupling.** Pi-core APIs are `peerDependencies: "*"`; pin a tested pi
   version range and add a smoke scenario that fails if `ExtensionAPI`/`setModel`/
   `registerProvider` signatures drift (the API is stable in 0.79.x but unversioned
   here).
5. **Credentialed validation still open.** The three gating unknowns from
   `model-mgmt-research.md` §6.3 (RPC streaming end-to-end, resume-after-kill,
   large tool-output truncation) remain unverified (no provider creds on the eval
   box) and now also cover plugin-in-the-loop behavior — P4b must run them.

> **Note on decomposition:** this task has a dedicated downstream consumer
> `pi-plugin-replan` whose job is to rebuild the task graph for the plugin-first
> direction. To avoid duplicate/conflicting task creation, this research
> **documents** the recommended replaces-vs-keeps and new work (§5) and leaves the
> actual `wg add` graph-building to `pi-plugin-replan`.

---

## Sources / artifacts examined

- **Pi 0.79.4 package** (`/tmp/pi-src/package`, from `npm pack @earendil-works/pi-coding-agent@0.79.4`):
  - `docs/extensions.md` — extension contract: events (`:272-872`), `ExtensionContext`
    (`:881-1011`), `ExtensionCommandContext` (`:1013-1207`), API methods
    (`registerTool :1269`, `sendUserMessage :1343`, `registerCommand :1424`,
    `registerShortcut :1496`, `registerFlag :1509`, `exec :1526`,
    `setActiveTools :1535`, `setModel :1562`, `setThinkingLevel :1576`,
    `events :1585`, `registerProvider :1594`), Mode Behavior table, Quick Start (`:55-100`).
  - `docs/sdk.md` — `createAgentSession` (`:25-68`), `AgentSession` incl.
    `setModel`/`cycleModel` (`:75-115`), `DefaultResourceLoader` +
    `extensionFactories`/`eventBus` (`:559-595`, `:831`).
  - `docs/packages.md` — pi-package manifest, `pi install` npm/git, `peerDependencies` rule.
  - `docs/rpc.md` — `set_model`/`cycle_model`, `extension_error`, Extension UI protocol (`:972-991`).
  - `examples/extensions/` (77 files): `tools.ts`, `dynamic-tools.ts`, `model-status.ts`,
    `handoff.ts`, `event-bus.ts`, `permission-gate.ts`, `custom-header.ts`/`custom-footer.ts`.
  - `dist/index.d.ts` (public exports), `dist/main.js:363,510,521`
    (pi's own modes built on `DefaultResourceLoader`/`extensionFactories`),
    `package.json` (`exports`, `jiti` dep).
  - `@earendil-works/pi-tui@0.79.9` `dist/terminal.js:80` (`setRawMode(true)`) — takeover mechanism (via `executor-research.md`).
- **Prior WG research (superseded for the in-session surface, reused for transport):**
  - [`executor-research.md`](executor-research.md) — takeover root cause, headless-launch fix, B1–B8, `pi:` handler sketch.
  - [`integration-plan.md`](integration-plan.md) — wrapper P0–P5 task breakdown (the replaces-vs-keeps target).
  - [`model-mgmt-research.md`](model-mgmt-research.md) — pi warm `set_model`, identity bridge.
  - [`upstream-patch/`](upstream-patch/README.md) — the staged (now-moot) `PI_NO_TUI` PR.
- **WG anchors:** `src/dispatch/plan.rs` (`ExecutorKind`, `EXTERNAL_CLIS`),
  `src/dispatch/handler_for_model.rs` (external-CLI interception),
  `src/commands/opencode_handler.rs` (RPC handler template),
  `src/tui/pty_pane.rs` (generic terminal-host PTY embed),
  `src/executor_discovery.rs`, `src/commands/service/ipc.rs` (`SetChatExecutor`).
- **Upstream:** https://github.com/earendil-works/pi · https://pi.dev/docs/latest · npm `@earendil-works/pi-coding-agent`.
