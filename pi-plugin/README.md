# @worksgood/wg-pi-plugin

The **integration channel** between [WG](../) and the [pi coding agent](https://pi.dev).
Loaded *inside* a pi session — the same artifact whether a human launched pi
(Topology C, auto-discovered) or WG spawned it (Topology A `pi --mode rpc`, or
Topology B the SDK Node host). See
[`docs/pi-integration/integration-plan-v2.md`](../docs/pi-integration/integration-plan-v2.md)
§2 and [`plugin-research.md`](../docs/pi-integration/plugin-research.md) §2.

## What it registers

| Surface | What |
|---|---|
| **Tools** (LLM/human callable) | `wg_ready`, `wg_show`, `wg_add`, `wg_done`, `wg_fail`, `wg_msg_send`, `wg_msg_read`, `wg_run` |
| **Commands** | `/wg ready\|graph\|show\|run\|add\|done\|fail`, `/wg-model <provider:id>` (warm in-session swap) |
| **Model bridge** | `registerProvider(WG endpoints/keys)` + managed-chat `model_select` → WG `CoordinatorState.model_override` write-back |

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
src/index.ts          registration entry — default export wgPlugin(pi)
src/tools.ts          the wg verb family
src/commands.ts       /wg and /wg-model (+ autocomplete)
src/model-bridge.ts   registerProvider + model_select write-back
src/wg-backend.ts     pi.exec("wg", …) client (daemon-IPC later)
host/wg-pi-host.mjs   Topology B: embed pi as a library with the plugin loaded
```

## Develop

```sh
npm install        # peer deps (pi-coding-agent/pi-ai/pi-tui) installed for dev
npm run build      # tsc → dist/ (no type errors)
npm test           # vitest unit tests (builds first)
npm run selftest   # node host/wg-pi-host.mjs --selftest → exit 0
```

Pi-core packages are `peerDependencies: "*"` (provided by pi at load) and are
**not** bundled. The package carries the `pi-package` keyword and points
`pi.extensions` at the built `dist/index.js`, so `pi install` / settings
`packages` can pull it.
