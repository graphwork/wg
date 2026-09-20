# @worksgood/pi

Connect Pi agents to WorksGood graphs, tools, and context.

The npm package is **`@worksgood/pi`** and Pi displays the extension as
**`pi-worksgood`**. The operational compatibility command remains
`wg pi-plugin`; it installs, inspects, and repairs the WorksGood integration.

This is the integration channel between [WorksGood](../) and the
[Pi coding agent](https://pi.dev).
Loaded *inside* a pi session — the same artifact whether a human launched pi
(Topology C, auto-discovered) or WG spawned it (Topology A `pi --mode rpc`, or
Topology B the SDK Node host). See
[`docs/pi-integration/integration-plan-v2.md`](../docs/pi-integration/integration-plan-v2.md)
§2 and [`plugin-research.md`](../docs/pi-integration/plugin-research.md) §2.

## What it registers

| Surface | What |
|---|---|
| **Tools** (LLM/human callable) | `wg_capabilities` (effective trusted/scoped/read-only policy), `wg_ready`, `wg_show`, `wg_add` (visible draft), `wg_publish` (explicit release), `wg_done`, `wg_fail`, `wg_msg_send`, `wg_msg_read`, `wg_run` |
| **Commands** | `/wg ready\|graph\|show\|run\|add\|done\|fail`, `/wg-viz` (live work-graph panel), `/wg-model <provider:id>` (warm in-session swap), `/wg-wake [on\|off]` (completion-event wakeups) |
| **VizView panel** | `/wg-viz` opens a read-only live work-graph panel via `ctx.ui.custom()` (TUI mode only): dependency tree, keyboard selection, expand/collapse, the TUI HUD's selected-task → detail-lines model, log tail; mouse clicks where the host pi-tui dispatches mouse to custom components. A passive widget (`ctx.ui.setWidget("wg-viz")`) shows a one-line in-progress/ready/blocked/done/failed summary. Data comes from the daemon IPC socket (read-only `viz_snapshot` request, bounded poll with change-guard); when the daemon is unavailable the panel falls back to the read-only `wg viz --all --no-tui` ASCII output and the widget silently clears. **Slice boundary (first slice):** deferred — filters, agency lanes UI, chat surfaces, back-edge arc rendering, scrollback search, and HUD-only detail sections (admission waiting, agency identity, route receipts). Read-only by construction: the panel never mutates graph state. |
| **Model bridge** | `registerProvider(WG endpoints/keys)` + managed-chat `model_select` → WG `CoordinatorState.model_override` write-back |
| **Completion wakeups** | A bounded poll of the graph (`wg list --json`, enriched with `wg show <id> --json`) tells the live session when a task reaches a terminal/needs-attention transition. The wake is injected with `pi.sendMessage` (`triggerTurn` when the session is idle; queued as a follow-up when busy) and pinged with `ctx.ui.notify`. A persisted per-session cursor (`<sessionFile>.wg-wake-cursor.json`) makes each transition announce exactly once and lets several sessions attached to one daemon keep independent cursors. Safe defaults: failures always, completions top-level only, `/wg-wake off` mutes a session. Read-only; the daemon-push/event-stream path is the documented follow-up. See [design-pi-completion-wakeups.md](../docs/design-pi-completion-wakeups.md). |

WG context (`WG_TASK_ID`, `WG_AGENT_ID`, `WG_CHAT_ID`, `WG_CHAT_REF`,
`WG_STATE_DIR`, `WG_DAEMON_SOCKET`, `WG_PROJECT_DIR`) rides in via environment
variables read inside the extension factory. `WG_CHAT_ID=.chat-N` is the
canonical persistence identity; `WG_CHAT_REF=chat-N` is accepted as a
compatibility alias and normalized to that task id. Without either explicit
chat variable the model bridge is inert: standalone Pi can cycle any provider's
models without invoking `wg`. Project cwd, `WG_DIR`, `WG_TASK_ID`, and Pi session
metadata never imply chat identity.

The backend shells the `wg` binary today
(`pi.exec("wg", …)`) and is structured to swap to a daemon-IPC client later
without touching the tool/command surface.

## Layout

```
src/index.ts          registration entry — default export worksgoodPi(pi)
src/tools.ts          the wg verb family
src/commands.ts       /wg and /wg-model (+ autocomplete)
src/viz-snapshot.ts   daemon-socket client: read-only viz_snapshot + bounded poller + ASCII fallback
src/viz-readmodel.ts  pure tree/detail/counts read-model (mirrors src/tui/viz_viewer/state.rs vocabulary)
src/viz-panel.ts      /wg-viz ctx.ui.custom() panel + setWidget live summary (TUI mode only)
src/model-bridge.ts   registerProvider + model_select write-back
src/completion-watcher.ts  completion-event wakeups: transition detection + deduped polling watcher + /wg-wake
src/wg-backend.ts     pi.exec("wg", …) client (daemon-IPC later)
host/wg-pi-host.mjs   Topology B: embed pi as a library with the plugin loaded
```

## VizView panel data path

The panel speaks the daemon's IPC protocol directly (`<wg-dir>/service/daemon.sock`;
one JSON-line `IpcRequest` per connection, one `IpcResponse` back —
`src/commands/service/mod.rs::send_request_to_socket_with_timeout`). The only
request it sends is `viz_snapshot` (served by
`src/commands/service/ipc.rs::handle_viz_snapshot`, projection in
`src/service/viz_snapshot.rs`), which is read-only and bounded: no descriptions
beyond a 512-byte head, no transcripts, no receipts, log tail clamped to 20.
Socket resolution: `WG_DAEMON_SOCKET` → `<WG_DIR>/service/daemon.sock` →
`<cwd>/.wg/service/daemon.sock`. Older daemons answer `unknown variant` — the
client treats that as offline and falls back to `wg viz` ASCII, so the panel is
forward-compatible both ways. The plugin-level test suite pins all of this
against a fixture daemon socket (`test/viz.test.ts`); the daemon-side contract
is pinned by the `pi_vizview_embedded_panel_contract` smoke scenario and the
`test_viz_snapshot_ipc_returns_live_graph_and_tracks_transitions` integration
test.

## Reloading a live chat session (`wg chat reload`)

After installing a new `wg` binary, a **live** pi chat keeps running whatever
plugin bundle it loaded when it spawned. `wg chat resume` respawns the handler
against the same `--session-dir` + `--session-id` (the conversation continues),
but it does not by itself re-materialize the embedded plugin cache. Use
`wg chat reload` for a full session-preserving reload:

```sh
wg chat reload <chat>      # one chat (numeric id, .chat-N, or name)
wg chat reload --all       # every active chat in this project
```

In order, a reload:

1. runs the `ensure-pi-plugin` primitive so the versioned cache matches this
   binary's embedded bundle (content-digest validated);
2. captures the **before** identity — wg/pi binary paths + mtime + content
   digest, plugin compat / source / resolved entry, embed + cache digests, and
   the pi session transcript;
3. signals the live handler to exit and lets the supervisor respawn it,
   resuming the **same** session dir/id;
4. waits for real liveness (the same bounded handler-lock/tmux proof
   `wg chat resume` uses — an accepted IPC is not success);
5. prints the **after** identity and the delta: changed/unchanged binaries,
   changed/unchanged plugin digest, session file preserved, and the transcript
   message count when it is cheap to read.

It **refuses loudly rather than degrading silently**: a plugin cache that is
still stale after `ensure-pi-plugin` (unrefreshable), a chat with no pi session
transcript, and a respawn that never becomes live within the bounded window each
produce a named error and leave the chat resumable with `wg chat resume <chat>`
(or stopped-but-resumable).

**Running it from inside the chat.** A reload takes the chat's own console down
on purpose: the handler process is the console. An agent can run
`wg chat reload` for its own session, but the command must come from **another
terminal or the TUI** (or the agent's own turn will be interrupted). The
conversation is preserved by the session file, so the session continues on the
respawned handler.

### Manual ritual (if you prefer hands-on recovery)

```sh
# 1. Re-materialize the embedded plugin cache for the running binary.
wg pi-plugin status          # inspect: source + embed/cache digest + cache state
wg pi-plugin install         # materialize + wire the console direction

# 2. Restart the chat handler against the same session.
wg chat resume <chat>        # stops + respawns; waits for real liveness

# 3. Confirm the console picked up the new extension.
wg chat show <chat>          # handler live?
wg pi-plugin status          # cache state should be "current"
```

`wg chat reload` is exactly this ritual, automated and with a before/after proof
that the plugin digest actually changed and the session file was preserved.

## Develop

```sh
npm install        # peer deps (pi-coding-agent/pi-ai/pi-tui) installed for dev
npm run build      # tsc → pi-worksgood/ (no type errors)
npm test           # vitest unit tests (builds first)
npm run selftest   # node host/wg-pi-host.mjs --selftest → exit 0
```

Pi-core packages are `peerDependencies: "*"` (provided by pi at load) and are
**not** bundled. The package carries the `pi-package` keyword and points
`pi.extensions` at the built `pi-worksgood/index.js`, so `pi install` / settings
`packages` can pull it.

## Legacy installs

`wg pi-plugin install` recognizes the former
`@worksgood/wg-pi-plugin` package and `…/wg/pi-plugin/…/dist/index.js`
settings. It retains legacy package records/version pins with their extension
resource disabled, replaces old managed paths with one compatible
`pi-worksgood/index.js`, and prints a one-time removal command. This avoids
duplicate tools and keeps offline consoles working during migration.
