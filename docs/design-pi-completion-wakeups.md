# Completion-event wakeups for the Pi plugin

**Status:** Implemented (minimal slice — polling watcher); daemon-push path documented as follow-up

**Date:** 2026-09-19

**Owner:** `completion-event-wakeups`

**Scope:** the `@worksgood/pi` extension (`worksgood-pi/`) attached to a WG
project while a human or agent Pi session is live.

**Normative neighbours:**
[`design-pi-plugin-install.md`](design-pi-plugin-install.md) (the embedded,
compat-locked plugin build) and
[`design-pi-session-watchdog.md`](design-pi-session-watchdog.md) (the
supervisor's process-epoch view of *Pi workers* — a different problem: that
design keeps a silent worker alive; this one tells a live chat session that the
graph changed).

---

## 1. Problem

A Pi session attached to a WG project learns about graph changes only when the
human (or the model, unprompted) asks. Today the plugin is *pull-only*:

* every `wg_*` tool and `/wg …` subcommand shells `wg` on demand
  (`worksgood-pi/src/wg-backend.ts`);
* `wg msg` / the chat inbox is **daemon-side** and is not delivered into a live
  Pi session;
* the VizView panel (`/wg-viz`, `worksgood-pi/src/viz-panel.ts`) is a *view*:
  it polls and re-renders, but it never surfaces a change to the conversation.

So a user who says "start the batch and I'll check back" has to keep asking
"is it done yet?". The missing surface is a **wake**: when a task reaches a
terminal (or needs-attention) transition, the live Pi session should be told,
with an actionable message, exactly once.

## 2. Event taxonomy

The watcher observes the graph as a set of `(task_id, status)` pairs. A **wake
transition** is a status change for one task between two consecutive reads.
Not every status change is worth a wake — the taxonomy below is deliberate.

### 2.1 Status classes

WG's canonical statuses (`src/graph.rs`, `enum Status`) group as:

| Class | Statuses | Meaning for the session |
|---|---|---|
| **Terminal success** | `done` | the deliverable landed |
| **Terminal failure** | `failed`, `abandoned` | the task will not proceed without intervention |
| **Needs attention** | `blocked`, `waiting`, `incomplete` | the task is stopped and a human decision or input is required |
| **In flight** | `open`, `in-progress`, `pending-validation`, `pending-eval`, `failed-pending-eval` | normal progress; **never** a wake on its own |
| **Internal** | any id beginning with `.` (`.chat-N`, `.evaluate-*`, `.flip-*`, `.assign-*`, completion-review satellites) | plumbing; never a wake |

A wake is emitted only for a transition whose **new** status is in one of the
first three classes. In-flight transitions are recorded in the cursor (so the
next read is compared against the right baseline) but produce no message.

### 2.2 Which tasks matter

Two knobs bound the volume:

1. **Failures are always surfaced.** A transition to `failed`/`abandoned` wakes
   regardless of position in the graph — the human needs to know the moment
   something breaks. This is the one non-negotiable default.
2. **Completions and attention are scope-filtered.** The default is
   `top-level`: only a *top-level* task wakes. A task is top-level when it has
   **no in-graph prerequisite** (`after` contains no task in the current
   snapshot) — i.e. it is a root of the dependency tree the VizView panel
   renders (`buildTree`, `worksgood-pi/src/viz-readmodel.ts`). In WG's
   decomposition convention a user's headline task is created first and
   subtasks are then added `--after <headline>`, so the headline is the root;
   subtask churn stays quiet unless a subtask *fails*.

The scopes are per class and configurable:

```text
completions = "top-level" | "all" | "off"     # default "top-level"
attention   = "top-level" | "all" | "off"     # default "top-level"
failures    = on (default)                     # off only via explicit opt-out
```

A shared **quiet** toggle mutes everything for one session.

Internal ids are excluded unconditionally — otherwise every agency one-shot
(which has no `after`) would look top-level and the user would be woken for
`.evaluate-*` chatter.

### 2.3 First read is a baseline

The first successful read of a session establishes the baseline and emits
nothing. Otherwise attaching to a long-running graph would replay every
historical `done` as if it just happened. A task that appears for the first time
*after* the baseline and is already terminal does wake (it was created and
finished between polls), with `from: "(new)"`.

## 3. The wake primitive (Pi side)

Two delivery surfaces, both already part of the pi 0.79+/0.85 extension API:

* **`pi.sendMessage(message, options)`** — appends a `CustomMessage` entry to
  the session. With `display: true` it is rendered in the conversation; with
  `triggerTurn: true` (and the agent idle) it **starts a new LLM turn**. When
  the agent is streaming, the message is queued and delivered as a follow-up.
  This is the message the *agent* sees and can act on.
* **`ctx.ui.notify(text, "info")`** — a lightweight, transient UI ping. It is
  not sent to the model; it is for the human's eyes. Only available when a UI
  exists (`ctx.hasUI`).

The watcher uses `sendMessage` as the primary surface and `notify` as a
best-effort companion when a UI is present.

### 3.1 Honest limit — a Pi session is turn-based

The plugin can make the information **present and visible**, and it can
**request** a turn, but it cannot make the model act. Precisely:

* If the session is **idle**, `sendMessage(..., { triggerTurn: true })` starts a
  turn: the agent will see the wake and can call `wg_show` / continue.
* If the session is **mid-turn (streaming)**, the wake is queued
  (`deliverAs: "followUp"`); the agent sees it when the current turn's queue is
  drained. It does not interrupt the in-flight tool call.
* If the session is **closed**, nothing is delivered. The cursor is persisted,
  so the next `session_start` in the same session file sees the transition on
  its first read (the baseline is per session file, not per process). A brand
  new session that never saw the old cursor starts from a fresh baseline and
  will **not** replay old transitions.
* A wake is a *notification*, never a lifecycle authority: it never claims,
  completes, fails, or mutates the graph. It is strictly read-only.

So "the pi chat is told when tasks finish" is true within an open session: the
message is in the session, displayed, and turn-triggering when idle. It is not
a pager that survives a closed terminal.

### 3.2 Message shape (actionable, never bare)

Every wake embeds the facts needed to act, plus the pointer to deeper detail:

```text
[WG] ✓ clarify-and-verify completed
Task: clarify-and-verify — Clarify and verify the thing
Summary: landed (disposition landed)
Receipt: b3:0cc468f5e550fa5889133069f83a3ed6ebbc5e2e3c632eeee47fa2c04fbeb050
Detail: call wg_show / run /wg graph, or open /wg-viz.
```

Failed and attention wakes swap `Summary`/`Receipt` for `Reason:` (from the
graph's `failure_reason`) or `Waiting on:` (the unfinished prerequisites). The
enrichment read (`wg show <id> --json`) is best-effort: if it fails, the wake
still carries id, title, status, and the detail pointer — never a bare
"something changed".

## 4. Subscription / dedup model

### 4.1 Cursor

Each watcher owns a **cursor**: `{ version, initialized, statuses: {taskId →
lastSeenStatus} }`. On every poll:

```
wakes, nextStatuses = planWakes(cursor.statuses, snapshotTasks, config)
emit wakes
if nextStatuses changed: persist cursor
```

Because the cursor stores the *last seen status* (not a set of announced
events), a repeated read of an unchanged graph produces no transition and
therefore no wake. Only an actual status change re-announces — including a
re-open followed by a second completion (`open → done → open → done` wakes
twice, which is correct: two distinct terminal transitions).

Ordering: emits happen **before** the cursor is persisted, so a crash between
the two can at worst deliver a duplicate wake (visible, harmless), never
silently drop one. Duplicate suppression is best-effort and documented as such.

### 4.2 Persistence

The default store is a JSON **sidecar next to the Pi session file**:
`<sessionFile>.wg-wake-cursor.json`, written atomically (temp file + rename).
This is deliberately keyed on the session, not the graph or the daemon:

* resuming the same Pi session keeps its cursor;
* a session with no file yet (early start, `rpc`-mode in-memory sessions) uses
  an in-memory store and simply loses its baseline across a restart — the next
  read becomes a fresh baseline, which suppresses rather than replays.

### 4.3 Multi-instance fan-out

Several Pi sessions can be attached to the same daemon/graph. Each session owns
its own cursor file (its own session file), so **each session independently
keeps its own baseline and announces a transition once for itself**. There is
no shared "announced" state and therefore no cross-session starvation: a
session started later sees the current state as its baseline and is not
retroactively spammed, while a session that was live across the transition gets
exactly one wake. This is intentionally *at-least-once per session*, not
exactly-once cluster-wide — the message is a notification, and a duplicate is
preferable to a silent miss.

## 5. Config surface

Plugin-side settings, read from the environment at factory time (the extension
already reads `WG_*` env this way) with safe defaults:

| Env | Values | Default | Effect |
|---|---|---|---|
| `WG_PI_COMPLETION_WAKES` | `on`/`off` | `on` | master switch for the watcher |
| `WG_PI_COMPLETION_FAILURES` | `on`/`off` | `on` | failures always (opt-out escape hatch) |
| `WG_PI_COMPLETION_COMPLETIONS` | `top-level`/`all`/`off` | `top-level` | completion scope |
| `WG_PI_COMPLETION_ATTENTION` | `top-level`/`all`/`off` | `top-level` | blocked/waiting scope |
| `WG_PI_COMPLETION_INTERVAL_MS` | integer ≥ 1000 | `15000` | poll cadence |
| `WG_PI_COMPLETION_QUIET` | `on`/`off` | `off` | per-session mute at start |

Invalid values fall back to the default (never throw). The **per-session quiet
toggle** is also a runtime command:

```
/wg-wake            # show effective config
/wg-wake on|off     # enable / mute this session
```

The quiet toggle is per session object, so `--quiet` in one session never
silences another. The watcher only runs in interactive/rpc sessions: a WG
worker's `json`/`print` session has no conversation to wake and must not be
interrupted by graph chatter.

## 6. Minimal slice (implemented)

`worksgood-pi/src/completion-watcher.ts`:

* `DEFAULT_COMPLETION_WAKE_CONFIG`, `readCompletionWakeConfig(env)` — §5.
* `planWakes(prevStatuses, tasks, config)` — pure transition detection, scope
  gating, internal-task filtering, baseline handling (§2, §4.1).
* `formatWakeMessage(wake, detail?)` — §3.2.
* `CompletionWatcher` — bounded poller (fixed interval, no overlapping
  in-flight reads, change-guarded cursor persistence) over an injected
  `readTasks` function; accepts an optional `enrich` hook and a `CursorStore`.
* `MemoryCursorStore`, `fileCursorStore(path)`.
* `installCompletionWatcher(pi, backend, env, options)` — wires
  `session_start` → start watcher + persist cursor, `session_shutdown` → stop,
  registers `/wg-wake`, and delivers each wake with `pi.sendMessage` +
  `ctx.ui.notify`.

Graph reads go through the **existing `WgBackend`** (`wg list --json`, with
`wg show <id> --json` for enrichment). No daemon protocol change is required —
the CLI/JSON lane already exists and is what every other plugin surface uses.

### 6.1 Follow-up: daemon push / event stream

Polling is the correct first slice, but it has a bounded cost (one `wg list
--json` per interval per session) and latency up to one interval. The
backend already notes the **`WG_DAEMON_SOCKET` IPC client as future work**
(`worksgood-pi/src/wg-backend.ts`), and the VizView panel already demonstrates a
read-only one-request/one-response daemon round trip
(`worksgood-pi/src/viz-snapshot.ts`, `viz_snapshot`). The follow-up is:

1. add a read-only daemon request that returns the same bounded projection, so
   the watcher stops shelling `wg list` per tick;
2. then a subscribe/event-stream lane so a transition is pushed instead of
   polled (the daemon already owns the transition; a subscription would emit
   on graph mutation and the watcher would keep its cursor purely for dedup).

Neither step changes the wake primitive, the cursor model, the config surface,
or the taxonomy — they only replace *how the snapshot arrives*.

## 7. Tests

`worksgood-pi/test/completion-watcher.test.ts` (vitest, credential-free, no
live daemon):

* **transition detection** — baseline emits nothing; top-level `done` wakes;
  child `done` stays quiet by default and wakes under `"all"`; failure of a
  child always wakes; `blocked`/`waiting` attention wakes top-level only;
  internal ids never wake; in-flight transitions never wake.
* **dedup / cursor persistence** — a repeated snapshot emits once; a
  re-open→re-complete announces again; `fileCursorStore` round-trips and
  tolerates a missing/corrupt file; the watcher bounds in-flight reads.
* **config gating** — quiet mutes everything; `off` scopes; invalid env falls
  back to defaults; `readCompletionWakeConfig` parsing.
* **message shape** — id, title, status, receipt/reason, and the wg-tools
  detail pointer are always present.
* **install** — a fake `ExtensionAPI` records the `session_start` /
  `session_shutdown` subscriptions and `/wg-wake`; a `session_start` over a
  fake backend produces exactly one `sendMessage` for one `done` transition and
  none on the next poll.

A real-graph integration check drives the actual `CompletionWatcher` (and the
installed watcher with a fake Pi API) against a scratch WG graph created with
the real `wg` binary (`wg init` in a temp git repo, `wg add` / `wg done`), with
a real exec host — no live daemon and no model credentials required. A true
`pi --mode rpc` scratch session is feasible only where a model credential is
available; the plugin's own load path is already pinned credential-free by the
`pi_vizview_embedded_panel_contract` smoke scenario, and the wake delivery is
pinned here against the real `sendMessage`-shaped API surface.

## 8. Non-goals / boundary

* No graph mutation. Reads only (`wg list` / `wg show`).
* No new WG status, event, or daemon protocol in this slice.
* No cross-session "announced once globally" ledger.
* No guarantee of an agent action — only that the information is present,
  visible, and turn-triggering when the session is idle (§3.1).
* No waking a closed session; the cursor makes the next live session
  consistent with itself, not with history.
