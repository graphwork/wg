# Pi plugin model cycling without a WG chat identity

> Historical naming note (2026-07-18): this report records behavior of the
> former `@worksgood/wg-pi-plugin` package and `pi-plugin/` component. The
> current integration is `@worksgood/pi`, displays as `pi-worksgood`, and lives
> under `worksgood-pi/`. Commands shown below are retained as incident evidence.

**Status:** fixed by `fix-pi-plugin` (2026-07-17)  
**Affected artifact:** `@worksgood/wg-pi-plugin` 0.1.0

## Intake evidence

In a standalone Pi TUI with the WorksGood extension loaded, pressing Ctrl-P
successfully changed the model and then printed a full exception stack. The
observed selections were OpenRouter Qwen models including
`qwen/qwen3.6-35b-a3b`, `qwen/qwen3.6-flash`, and
`qwen/qwen3.6-max-preview`:

```text
wg-pi-plugin: model write-back failed for openrouter:qwen/qwen3.6-flash: Error: wg-pi-plugin: cannot write model override — no chat id (set $WG_CHAT_ID)
    at WgBackend.setModelOverride (.../wg-pi-plugin/0.1.0/dist/wg-backend.js:126:19)
    at .../wg-pi-plugin/0.1.0/dist/model-bridge.js:116:27
    at ExtensionRunner.emit (.../pi-coding-agent/dist/core/extensions/runner.js:548:49)
    at AgentSession._emitModelSelect (.../pi-coding-agent/dist/core/agent-session.js:1188:37)
    at AgentSession._cycleAvailableModel (.../pi-coding-agent/dist/core/agent-session.js:1270:20)
```

The same noise was not noticed while using a local llama.cpp model. The intake
correctly identified the missing WG chat context as the likely cause and warned
not to patch the generated cache under `~/.cache/wg/pi-plugin/0.1.0/`.

## Root cause

Pi emits `model_select` after `/model`, `/wg-model`, Ctrl-P cycling, and session
restore. The extension subscribed in every Pi topology, including a human-run
standalone console. Its listener called `WgBackend.setModelOverride()` for every
non-restore event. The backend then threw when `WG_CHAT_ID` was absent, and the
listener passed the `Error` object to `console.error`, which rendered its stack.

Provider selection was never the eligibility boundary. The listener is invoked
for both `openrouter:*` and local `llamacpp:*` models. The reported difference
can therefore only be incidental—for example, the local observation used a
restore event (already skipped), a Pi process without the extension, or did not
notice/reproduce the emitted line. It is not evidence of different persistence
semantics. Regression coverage emits identical `cycle` events for both provider
families.

## Fix and contract

Persistence eligibility is now based only on an explicit, addressable WG chat
launch identity:

- canonical: `WG_CHAT_ID=.chat-N` (legacy `.coordinator-N` remains readable);
- compatibility alias: `WG_CHAT_REF=chat-N`, normalized to `.chat-N`;
- absent or malformed identity: standalone mode, so model write-back is a silent
  no-op with no `wg` process and no console error;
- no fallback from `WG_TASK_ID`, cwd, `WG_DIR`, project state, or Pi session id.

The guard exists at the `model_select` event boundary and again in the backend.
A managed write uses one command targeting the exact task:

```text
wg chat model .chat-N <provider:model> --warm-pi-writeback
```

WG records a handler-first `pi:<provider>:<model>` override without terminating
the already-warm Pi process. Repeated identical writes are idempotent. A genuine
managed-chat failure remains visible as one concise, actionable line; the
extension does not pass an `Error` object and therefore does not duplicate stack
output.

All WG Pi chat launch surfaces now export both variables consistently: the TUI
PTY path, daemon/supervisor `spawn-task` path, direct interactive Pi dispatch,
and hermetic `wg pi-handler` RPC/Node-host paths.

## Artifact policy

The source of truth is `pi-plugin/src/`. After the source fix, run
`make embed-pi-plugin` to regenerate committed `pi-plugin/embedded/`. Never edit
the versioned cache in place. This repair changes context gating and launch env and adds the plugin-only
`--warm-pi-writeback` CLI contract. An older WG binary cannot honor that
request, so `WG_PI_PLUGIN_COMPAT_VERSION` is bumped from 0.1.0 to 0.1.1. The
exact-match handshake now rejects either skew direction, and the versioned
cache rematerializes the corrected embedded bytes.
