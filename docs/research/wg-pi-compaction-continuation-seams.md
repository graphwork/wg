# WG Pi compaction-continuation seams audit

**Status:** current-code audit; no fix implemented

**Scope:** generated task-worker wrapper → `pi --mode json` → `@worksgood/pi` → native stream observer → `PiWatchdog` → lifecycle/convergence/finalization.

## Executive finding

WG does **not** currently cause a real Pi turn after a threshold compaction. The native parser has no cases for `compaction_start`, `compaction_end`, `queue_update`, `auto_retry_*`, or the current `summarization_retry_*` names; its only compaction case is the older `compaction_retry` spelling (`PiWatchdog::ingest_native_value`, `src/pi_watchdog/mod.rs:988-1081`). When an eventual `agent_settled` is observed, the watchdog reserves a continuation epoch, renders and hashes the stock prompt, and appends a `type:"custom"` marker that contains only the action metadata—not the rendered prompt (`PiWatchdog::needs_finalization`, `src/pi_watchdog/mod.rs:1676-1717`; `emit_completion_prompt`/`append_session_marker`, `src/pi_watchdog/mod.rs:1719-1781). Production callers discard the returned `ActionKind::LaunchSameSession` and `ActionKind::AppendCompletionPrompt` (`observe_live`, `src/commands/pi_stream_bridge.rs:33-111`; finished-stream replay, `src/commands/pi_stream_bridge.rs:137-201`; `process_exit`, `src/commands/pi_watchdog.rs:907-961). Therefore the watchdog records state/session/lifecycle markers only; it neither invokes Pi nor delivers an LLM-context message.

The correct split is:

* **Authority and idempotency:** the lifecycle kernel plus `PiWatchdog`, because they hold the exact source attempt, process epoch, session proof, route snapshot, terminal CAS, and finite continuation budget (`PiWatchdogState`, `src/pi_watchdog/mod.rs:440-493`; `PiContinuationEpochReserved`, `src/lifecycle.rs:1079-1147).
* **Delivery:** the embedded extension inside the already-running Pi process, using Pi's context-bearing `sendMessage(..., { deliverAs: "followUp", triggerTurn: true })` (Pi runtime contract, `@earendil-works/pi-coding-agent/docs/extensions.md:1388-1410`), after an authenticated WG claim. The current entry point wires tools, commands, model write-back, and a shutdown hook, but wires no continuation component (`worksgood-pi/src/index.ts:75-89`).

The wrapper and raw observer should remain launch/reap and evidence transports. They cannot safely deliver into a one-shot JSON process. Migrating the worker to RPC could create a bidirectional delivery harness, but it is a larger transport replacement, and the existing RPC chat adapter is not attempt-authoritative and stops at `agent_end` rather than `agent_settled` (`RpcTransport::send_turn`, `src/commands/pi_handler.rs:377-444`).

## Evidence baseline

The embedded extension intentionally peers against whatever Pi host loads it (`worksgood-pi/package.json:31-39`), so the current host event contract—not the plugin's development version—is relevant. Current Pi documents `queue_update`, `compaction_start/end`, `auto_retry_*`, and `summarization_retry_scheduled/attempt_start/finished` as JSON/RPC events (`@earendil-works/pi-coding-agent/docs/json.md:14-27`; `docs/rpc.md:837-860,1016-1126`) and defines `agent_settled` as the boundary after retry, compaction retry, and queued continuation are exhausted (`docs/rpc.md:837-887`; `docs/extensions.md:558-571`). The ExtensionAPI contract is narrower but sufficient for the proposed trigger pair: `session_compact` supplies the saved compaction plus `reason` and `willRetry`, then `agent_settled` proves no automatic retry/compaction/follow-up remains (`@earendil-works/pi-coding-agent/docs/extensions.md:514-543,558-571`). It does **not** expose the raw `queue_update` or summarization-retry events documented for JSON/RPC; those remain observer diagnostics, not plugin trigger inputs. The repository's plugin lock currently resolves its development host to 0.79.10 while the peer range remains open (`worksgood-pi/package-lock.json:565-590`; `worksgood-pi/package.json:31-39`), so production code must gate/test the supported host contract explicitly rather than assume every peer version has both hooks.

## Current sequence

```mermaid
sequenceDiagram
    participant D as WG spawn/daemon
    participant W as generated run.sh wrapper
    participant P as pi --mode json
    participant X as @worksgood/pi extension
    participant O as pi-stream-observe
    participant WD as PiWatchdog
    participant K as Lifecycle kernel
    participant C as convergence/finalizer

    D->>W: gated spawn + opaque attempt capability
    Note over D,W: spawn setup/capability — src/commands/spawn/execution.rs:1630-1706,1805-1880
    W->>P: cat prompt.txt | pi --mode json --session-id/dir
    Note over W,P: argv/stdin — src/service/executor.rs:1729-1746; src/commands/spawn/execution.rs:2741-2784,2808-2818
    W->>WD: bootstrap(child PID, wrapper PID, metadata, session plan)
    WD->>K: PiContinuationAuthorized(source/process/session/route)
    Note over WD,K: bootstrap — src/commands/pi_watchdog.rs:539-723
    W->>O: follow raw_stream.jsonl for exact child PID
    Note over W,O: wrapper observer — src/commands/spawn/execution.rs:3500-3524
    P-->>O: JSON events
    opt extension was discoverable from ambient Pi settings
        P->>X: load tools/commands/model bridge
        Note over P,X: wiring — worksgood-pi/src/index.ts:75-89; worker defaults — src/service/executor.rs:1729-1746
    end
    P-->>O: compaction_start/end, queue_update, summarization_retry_*
    O->>WD: ingest_native_line(...)
    Note over O,WD: exhaustive parser — src/pi_watchdog/mod.rs:988-1081; these names are inert
    P-->>O: agent_settled
    O->>WD: AgentSettled
    WD->>WD: completion_handoff + reserve epoch + append custom marker
    WD->>K: sync pi_continuation_epoch
    Note over WD,K: settled/outbox — src/pi_watchdog/mod.rs:1338-1357,1676-1781; src/commands/pi_watchdog.rs:249-310
    WD-->>O: [Reserve, LaunchSameSession, AppendCompletionPrompt]
    Note over WD,O: discard — src/commands/pi_stream_bridge.rs:70-91; no Pi API call
    P-->>W: process exits
    W->>WD: finished replay + ProcessExited
    Note over W,WD: discard/reap — src/commands/pi_stream_bridge.rs:176-190; src/commands/pi_watchdog.rs:947-961
    W->>K: fail if task still InProgress
    Note over W,K: wrapper terminal check — src/commands/spawn/execution.rs:3696-3711
    C->>K: after exact owner death, request reopen preserving session/worktree
    K->>D: new generation/new attempt dispatch
    Note over C,D: convergence — src/commands/finalize.rs:1549-1702; tests/smoke/scenarios/exited_worker_finish_convergence.sh:115-149
```

### Launch and extension loading

The built-in task executor is `pi --mode json -p ...`; it is explicitly described as a one-shot worker surface, distinct from the long-lived RPC chat surface (`ExecutorConfig::builtin("pi")`, `src/service/executor.rs:1729-1746`). The command builder creates or resolves an exact session ID/file, writes `pi-session-plan.json`, and passes `--session-dir` plus `--session-id` before piping `prompt.txt` to stdin (`external_prompt_command`, `src/commands/spawn/execution.rs:2741-2784,2808-2818`; `prompt_file_command`, `src/commands/spawn/mod.rs:31-38`).

The generated Pi wrapper starts the child behind `pi-bootstrap.gate`, bootstraps the watchdog with child and wrapper PIDs, tails the raw stream, waits for both child and observer, then runs the finished bridge (`write_wrapper_script`, `src/commands/spawn/execution.rs:3500-3524,3665-3678`). This ordering gives the observer a live exact-process window, but the only stdin delivery is the initial `cat prompt.txt | pi ...`; JSON mode is not a command channel (`src/commands/spawn/mod.rs:31-38`; `src/service/executor.rs:1729-1746`).

The task-worker command does **not** explicitly add `-e <embedded>/index.js`, disable discovery, or inject the plugin compat handshake: the built-in Pi args contain only mode/prompt, while `external_prompt_command` adds model, reasoning, and session flags (`src/service/executor.rs:1729-1746`; `src/commands/spawn/execution.rs:2700-2784`). Setup/profile activation may wire the cached embedded plugin into global Pi settings (`src/commands/setup.rs:2688-2711`; `src/commands/profile_cmd.rs:1160-1188`), so the extension may load ambiently, but direct Pi route configuration has no task-worker JIT `ensure_pi_plugin` call. By contrast, the chat RPC path does materialize the embedded plugin, passes `-e` plus `-ne`, and injects the compat environment (`src/commands/pi_handler.rs:497-539,850-925,1010-1048`). Thus “embedded plugin in the worker path” is conditional today, not a hermetic invariant.

When loaded, the plugin has a real Pi `ExtensionAPI`, but its entry point subscribes only to `session_shutdown` and imports/wires exactly the tools, commands, and model-bridge modules shown there (`worksgood-pi/src/index.ts:15-21,75-89`). Command setup observes `session_start`, and the model bridge observes `model_select`; none of those wired registrations is a compaction/settled continuation handler (`worksgood-pi/src/commands.ts:51-61`; `worksgood-pi/src/model-bridge.ts:128-155`).

## Native projection audit

`ingest_native_line` advances a per-capture cursor in memory and then calls `ingest_native_value`; only the latter persists recognized observations or changed bounded activity (`src/pi_watchdog/mod.rs:839-864,988-1090`). Consequently an unknown tail event is semantically inert and its new cursor is not durable until a later recognized event causes persistence.

| Native record | Current projection | Consequence | Evidence |
|---|---|---|---|
| `turn_start` / request-start aliases | `ProviderRequestStarted`; phase and meaningful clock advance | projected | `src/pi_watchdog/mod.rs:998-1010,1287-1294` |
| `message_start` / response-start aliases | `ProviderResponseStarted` | projected | `src/pi_watchdog/mod.rs:1011-1014,1294-1298` |
| `message_update` thinking/text/tool-call deltas | bounded activity plus `ThinkingDelta`/one-token `TokenDelta` | projected without raw text | `src/pi_watchdog/mod.rs:1017-1030,1124-1188` |
| tool start/update/end | tool/effect state and bounded activity | projected; start is conservatively non-idempotent until an end receipt | `src/pi_watchdog/mod.rs:1031-1069,1189-1234,1370-1411` |
| `turn_end` | deduplicated usage totals and `UsageReceipt` | projected only when usage changed | `src/pi_watchdog/mod.rs:1070-1075,1235-1281` |
| `provider_retry` | `ProviderRetry` | projected, but current `auto_retry_start/end` names are not | `src/pi_watchdog/mod.rs:1015-1017,1298-1301` |
| `compaction_retry` | `CompactionRetry` | only this older spelling is projected | `src/pi_watchdog/mod.rs:1016-1018,1300-1302` |
| `compaction_start` / `compaction_end` | none | reason, result/aborted, and `willRetry` are lost | exhaustive match, `src/pi_watchdog/mod.rs:998-1081,1124-1281` |
| `queue_update` | none | WG cannot distinguish pending follow-up/steering from an empty queue; `Observation::QueuedFollowUp` has no native constructor | enum at `src/pi_watchdog/mod.rs:593-644`; exhaustive match at `src/pi_watchdog/mod.rs:998-1081` |
| `summarization_retry_scheduled` / `summarization_retry_attempt_start` / `summarization_retry_finished` | none | retry delay/attempt activity does not reset meaningful silence or phase | exhaustive match, `src/pi_watchdog/mod.rs:998-1081,1124-1281` |
| ordinary `agent_end` | bounded `native_activity` only | not settled/final; only `willRetry:true` becomes `AgentEndWillRetry` | `src/pi_watchdog/mod.rs:1076-1080,1126-1130,1302-1305` |
| `agent_settled` | `CompletionHandoff`, `NeedsFinalization`, continuation reservation/marker | semantic-neutral handoff is durable, but returned launch/delivery actions are not executed | `src/pi_watchdog/mod.rs:1079-1081,1338-1357,1676-1717` |

Persisting full queue strings, provider errors, summaries, or reasoning would violate the existing bounded-projection boundary: `NativeActivityProjection` deliberately has numeric/category fields and hashed stream identities, not provider text (`src/pi_watchdog/mod.rs:38-91,993-996`). A fix should project only queue counts/non-empty bits, compaction reason/outcome, retry attempt numbers, and stable action/entry digests.

## Action delivery audit

`ActionKind` names behavior that the current state machine does not perform externally: `LaunchSameSession` and `AppendCompletionPrompt` are enum values (`src/pi_watchdog/mod.rs:552-560`). `needs_finalization` returns them after it has already reserved an epoch and called `emit_completion_prompt` (`src/pi_watchdog/mod.rs:1676-1717`).

`emit_completion_prompt` renders `STOCK_PROMPT_TEMPLATE`, hashes it, persists a prompt intent, and appends a session marker if absent (`src/pi_watchdog/mod.rs:10-17,1719-1752`). The appended JSON contains `type`, `customType`, action/version/reason/digest, and epoch numbers; it contains neither `prompt` nor `content`, and it is written directly with `OpenOptions` rather than Pi's session manager (`src/pi_watchdog/mod.rs:1761-1781`). Pi's session format defines custom entries as extension state excluded from LLM context and custom messages as the context-bearing form (`@earendil-works/pi-coding-agent/docs/session-format.md:263-286,411-416`; `docs/extensions.md:1388-1449`). The current marker is therefore evidence/idempotency metadata, not delivery—even before considering that its shape lacks Pi-managed `id`, `parentId`, `timestamp`, and `data` fields.

All production action-return sites discard the vector:

1. The live observer calls `ingest_native_line(...)?` only for its side effects and never binds the returned vector (`src/commands/pi_stream_bridge.rs:70-91`).
2. Finished replay uses `let _ = watchdog.ingest_native_line(...)` (`src/commands/pi_stream_bridge.rs:176-190`).
3. `process_exit` calls `watchdog.observe(...)` and discards its returned value (`src/commands/pi_watchdog.rs:947-961`).
4. Production has no caller of `PiWatchdog::tick`; repository callers are integration tests and the fixture CLI (`src/commands/pi_watchdog.rs:1229-1238`; `tests/integration_pi_watchdog.rs:163-188`).

Tests currently assert the vectors and marker count, not an actual Pi turn: `settled_and_every_exit_need_finalization_not_terminal` expects the three actions and one prompt count without a Pi process (`tests/integration_pi_watchdog.rs:245-280`). This is positive evidence that the existing test boundary is state-machine output only.

### Reproducible exhaustive searches for negative claims

These repository-wide/source-scope searches were run against the exact candidate tree (`git rev-parse HEAD` is recorded with the immutable validation evidence):

```text
$ rg -n 'watchdog\.tick|PiWatchdog::tick|\.tick\(now' src --glob '*.rs'
src/commands/pi_watchdog.rs:1233:    let actions = watchdog.tick(now).map_err(anyhow::Error::new)?;

$ rg -n 'ensure_pi_plugin|WG_PI_PLUGIN_COMPAT_VERSION|\.arg\("-e"\)' \
    src/commands/spawn/execution.rs src/service/executor.rs
(no matches; exit 1)

$ rg -n 'LaunchSameSession|AppendCompletionPrompt' src tests --glob '*.rs'
tests/integration_pi_watchdog.rs:269-270: assertions
src/pi_watchdog/mod.rs:557-558: enum declarations
src/pi_watchdog/mod.rs:1714-1715: returned vector

$ rg -n 'pi\.on\(' worksgood-pi/src --glob '*.ts'
worksgood-pi/src/commands.ts:55: session_start
worksgood-pi/src/index.ts:87: session_shutdown
worksgood-pi/src/model-bridge.ts:135: model_select

$ rg -n 'compaction_start|compaction_end|queue_update|summarization_retry|auto_retry|compaction_retry|agent_settled|agent_end' src/pi_watchdog/mod.rs
src/pi_watchdog/mod.rs:1011: compaction_retry
src/pi_watchdog/mod.rs:1076: agent_end when willRetry=true
src/pi_watchdog/mod.rs:1079: agent_settled
src/pi_watchdog/mod.rs:1126: bounded agent_end activity
```

The first result is the fixture command already cited at `src/commands/pi_watchdog.rs:1229-1238`; there is no production scheduler call. The empty second result is scoped to the two task-worker argv construction files, while positive hermetic `ensure_pi_plugin`/`-e` evidence is in the separate chat RPC path (`src/commands/pi_handler.rs:497-539,850-925`). The remaining searches independently reproduce the action-consumer, plugin-registration, and native-event match inventories.

## Authority available at each candidate seam

| Candidate | Current process/session/route/attempt proof | Can deliver a real same-process turn now? | Assessment |
|---|---|---|---|
| **Embedded extension** | In-process Pi context supplies current session manager/leaf, current model, mode, and idle state; the worker environment supplies `WG_TASK_ID`, `WG_MODEL`, and opaque `WG_WORKER_CAPABILITY`, but `readWgEnv` currently retains only task/agent/chat/state/socket/dir (`worksgood-pi/src/wg-backend.ts:28-93`; worker env/capability at `src/commands/spawn/execution.rs:1630-1706,1848-1880`). It does not itself possess the WG generation/attempt/fence/process digest. | **Yes**, if loaded: `sendMessage`/`sendUserMessage` are Pi-native delivery APIs; current `/wg run` already demonstrates `sendUserMessage` (`worksgood-pi/src/commands.ts:88-102`). | Best delivery owner, not sufficient authority alone. Must claim through the daemon using the opaque attempt capability. |
| **Generated wrapper** | Owns wrapper PID, child PID, task/agent/run/model env, and paths to metadata/session plan; metadata includes generation, attempt ID/fence, lease, route, run ID, and worktree (`src/commands/spawn/execution.rs:2029-2068`). Bootstrap proves child-of-wrapper and captures kernel identity (`src/commands/pi_watchdog.rs:539-723,725-786`). | **No** for the current live JSON child. Its stdin is the one-shot initial prompt and EOF; post-child bridge/reap is too late (`src/commands/spawn/mod.rs:31-38`; `src/commands/spawn/execution.rs:3500-3524,3665-3678`). | Keep as launch/reap supervisor. Restarting Pi here is a process replacement and conflicts with the requested same-attempt in-process kick. |
| **JSON raw observer/bridge** | `open_watchdog_for_agent` requires metadata attempt ID equal the lifecycle current attempt, reconstructs the exact runtime key, checks task/generation/attempt/fence/lease, syncs process authority, and the follower checks `follow_pid` against the watchdog PID (`src/commands/pi_stream_bridge.rs:238-287,55-69`). | **No.** It has read access to stdout capture and WG state, but no Pi command/API channel. | Strong evidence seam; wrong delivery seam. It should project bounded compaction/queue/retry evidence, not manufacture a turn. |
| **Existing RPC harness** | `RpcTransport` owns the child/stdin/stdout, request IDs, explicit provider/model/reasoning, session ID/dir, and hermetic plugin path (`src/commands/pi_handler.rs:340-405,497-539`). Current chat RPC environment binds a chat and graph, not a worker generation/attempt/fence/process authorization (`src/commands/pi_handler.rs:1010-1048`). | **Yes**, through a correlated `prompt` command, but current `send_turn` exits its read loop at `agent_end`, not `agent_settled` (`src/commands/pi_handler.rs:377-444`; accumulator at `src/commands/pi_handler.rs:172-211`). | Viable only after replacing the task-worker transport and adding lifecycle authority. Larger than necessary for a compaction kick. |
| **`PiWatchdog` + lifecycle** | Holds `SourceTuple` (task/generation/attempt/fence/lease/worktree), `RouteSnapshot`, `SessionProof`, exact PID/PGID/start ticks/boot/nonce, process and continuation epochs, guards, terminal receipt, budgets, and action receipts (`src/pi_watchdog/mod.rs:213-286,440-493`). Bootstrap binds these to the current lifecycle attempt and records a finite authorization (`src/commands/pi_watchdog.rs:539-723`). | **No.** It can authorize/record but has no Pi `ExtensionAPI` or RPC stdin. | Correct authority/idempotency owner; wrong delivery owner. |

Route proof is not fully cryptographic today. Bootstrap freezes provider/model/reasoning from metadata, but fills endpoint as `pi-owned`, hashes the model string for `endpoint_hmac`, uses `pi-path-owned` for the binary, and stores the plugin compat version rather than an artifact digest (`src/commands/pi_watchdog.rs:617-649`). Session and process proof are stronger: bootstrap validates the planned header/prefix/canonical journal and captures PID birth identity plus wrapper ancestry (`src/commands/pi_watchdog.rs:558-616,650-723,725-786`). Any fix should preserve the exact selected route from the attempt and fail closed on model/session drift, while not claiming the current placeholder fields prove more than they do.

## Terminal, wait, epoch, exit, and convergence constraints

### First terminal wins

The lifecycle kernel refuses continuation reservation after `pi_terminal_reservation` exists, verifies the current process identity and continuation CAS, and atomically charges the finite budget (`src/lifecycle.rs:1079-1147`). `PiTerminalIntent` verifies process plus source tuple, refuses a second terminal, stores the first receipt, and consumes the continuation authorization (`src/lifecycle.rs:1183-1220`). The watchdog independently makes a terminal receipt sticky, cancels pending actions, and rejects a contradictory later terminal (`src/pi_watchdog/mod.rs:1278-1290,1787-1840`).

A post-compaction claim must therefore occur through the same lifecycle transaction, not by trusting an event or session marker. The cross-process graph-CAS → Pi-API boundary cannot make “actual message enqueue” atomic with a concurrent terminal receipt. The protocol must expose that fact rather than claim otherwise: a terminal/park that wins before a **delivery-permit CAS** cancels the outbox; if the delivery permit wins first, that one kick is already authorized and a later terminal prevents every later permit/epoch (and should abort the just-started run where Pi still permits it), but does not retroactively reorder the committed permit. An uncertain crash between permit and Pi acknowledgement must hold/reconcile, never blindly redeliver. In either ordering the kick stays on the same attempt and process fence (`src/lifecycle.rs:1079-1220`; `src/pi_watchdog/mod.rs:1278-1290,1842-1862`).

### Wait/park

`wg wait` attests the active Pi continuation against the watchdog's source, session digest, route digest, process epoch/identity, terminal-clear guards, and optional `PI_SESSION_ID`, then persists the exact session selector in the same transaction as `AttemptParked` (`src/commands/wait.rs:160-235,262-326`). `AttemptParked` terminalizes the current attempt and changes status to `Waiting`; another worker/process transition must satisfy the running-attempt fence and is rejected afterward (`src/lifecycle.rs:803-809`; `require_running_attempt`/first-terminal logic, `src/lifecycle.rs:1445-1474`).

`Observation::WaitAccepted` also suppresses watchdog ticks by setting `WaitingUser` and cancelling pending actions, although current production `wg wait` does not feed that observation directly (`src/pi_watchdog/mod.rs:1412-1427,1477-1480`; only fixture construction at `src/commands/pi_watchdog.rs:1148-1151`). The lifecycle status/fence is therefore the authoritative suppression; plugin-local idle/queue state is not enough.

### Continuation budget without a retry domain

A same-process prompt increments `continuation_epoch` but intentionally leaves `process_epoch` unchanged (`PiWatchdog::reserve_epoch`, `src/pi_watchdog/mod.rs:1842-1862`; lifecycle field contract, `src/lifecycle.rs:198-209`). Both watchdog and lifecycle cap epochs and reserved elapsed time (`src/pi_watchdog/mod.rs:112-145,1863-1890`; `src/lifecycle.rs:1122-1147`). The kick should consume this existing budget exactly once. It must not increment source retry, admission, breaker, evaluation, or accounting domains; those counters are distinct state (`DomainCounters`, `src/pi_watchdog/mod.rs:429-438`).

### Wrapper exit and convergence

Delivery must be accepted by the still-live Pi process before wrapper reap. Once the child exits, the wrapper runs bridge/reap, records `PiProcessEpochExited`, and fails a still-`InProgress` Pi task (`src/commands/spawn/execution.rs:3665-3711`; `src/commands/pi_watchdog.rs:907-961`). A marker written after exit cannot cause that dead process to turn.

The current service fallback detects a dead exact owner with no finish transaction, uses `completion_handoff` plus wrapper/child capability proof, and requests an exact-session/worktree reopen (`src/commands/finalize.rs:1549-1702`). That fallback is intentionally a new generation/attempt: the smoke scenario asserts two Pi launches, `generation == 1`, and `attempt_sequence == 2` (`tests/smoke/scenarios/exited_worker_finish_convergence.sh:115-149`). The post-compaction kick must happen before this fallback and leave lifecycle status `InProgress`, the current source attempt/fence/worktree unchanged, process epoch unchanged, and only the continuation epoch advanced. Existing convergence then remains the safety net for a genuine process exit, not the kick mechanism.

## Recommended ownership and protocol shape

### 1. Watchdog/lifecycle owns an authenticated continuation outbox

Add worker-control claim/permit/ack operations for one compaction-kick outbox. The daemon should derive the source tuple from the opaque worker capability, load the current lifecycle and watchdog, and require all of the following before reserving the existing continuation epoch:

* current task/generation/attempt/fence/worktree lease match (`checked_open`, `src/commands/pi_watchdog.rs:92-118`);
* current process epoch/identity and terminal wrapper binding match (`sync_lifecycle_process_authority`, `src/commands/pi_watchdog.rs:119-208`);
* session ID/file/leaf and frozen route still match authorization (`src/commands/wait.rs:160-235` is the existing strict pattern);
* task is still running, no terminal reservation or park has won, no unsafe/open tool effect exists, and finite continuation budget remains (`src/lifecycle.rs:1079-1147`; `src/pi_watchdog/mod.rs:1676-1703`);
* trigger is a successful, non-retrying threshold compaction plus a settled/idle delivery boundary, represented by bounded event/entry IDs—not raw summary/queue text.

The outbox key should include at least attempt ID/fence, process epoch/identity digest, prior continuation epoch, session ID plus compaction-entry/leaf identity, prompt version, and prompt digest. Use explicit `Authorized → DeliveryPermitted → Acknowledged` (or `Cancelled`/`Uncertain`) states. `DeliveryPermitted` is the linearization point that races the lifecycle terminal/park CAS: it rechecks the current tuple and terminal-clear state immediately before the plugin invokes Pi. Replays return the same action/state and never reserve another epoch. A crash after permit but before acknowledgement is `Uncertain` and requires Pi-session reconciliation; absence of proof is not permission to send again.

### 2. Embedded extension owns delivery and acknowledgement

Of the audited components, the extension is the narrow existing seam with both (a) Pi-native completed-compaction/settled hooks and (b) a Pi-native context delivery API: `session_compact` reports `reason`/`willRetry`, `agent_settled` is the no-autonomous-work boundary, and `sendMessage` can trigger an idle turn (`@earendil-works/pi-coding-agent/docs/extensions.md:514-543,558-571,1388-1410`). It does not receive raw queue/retry events; the trigger is specifically “remember successful threshold `session_compact` with `willRetry == false`, then claim at `agent_settled`.” It should request the authenticated outbox action through `WgBackend` (which already shells `wg` under the worker capability, `worksgood-pi/src/wg-backend.ts:96-131`), then use a hidden/custom context message with the returned stock prompt and action ID. `sendMessage` is preferable to a forged human `sendUserMessage`: it can carry a distinct `customType`/details for idempotency while still participating in LLM context (`@earendil-works/pi-coding-agent/docs/extensions.md:1388-1410,1563-1576`). If the supported Pi host cannot guarantee both extension hooks, this recommendation fails closed and the next candidate is an attempt-authoritative RPC worker harness—not inference from the raw observer.

Delivery acknowledgement must be separate from authorization, and the plugin must obtain the terminal-racing `DeliveryPermitted` CAS immediately before invoking Pi. On restart/reload, it should inspect Pi-managed session entries for the action ID and reconcile the WG outbox: already-present context message → acknowledge without redelivery; authorized but not permitted + current tuple → request a permit; permitted but message absent → mark uncertain/hold rather than assume non-delivery; terminal/park/budget/session/route mismatch → cancel/hold. Do not append raw JSON directly to Pi's session file; use Pi's session manager API so tree IDs/parents and in-memory context agree (`@earendil-works/pi-coding-agent/docs/session-format.md:169-286,379-416`).

### 3. Explicitly load the extension for task workers

The JSON task command should mirror the RPC handler's hermetic plugin guarantee: `ensure_pi_plugin(EnsureMode::Hermetic)`, `-e <exact embedded entry>`, discovery policy chosen deliberately, and `WG_PI_PLUGIN_COMPAT_VERSION` injected (`src/commands/pi_handler.rs:497-539,850-925,1010-1048`). Depending on profile/setup side effects is insufficient because the current task-worker builder has no explicit plugin path (`src/service/executor.rs:1729-1746`; `src/commands/spawn/execution.rs:2700-2784`).

### 4. Raw observer records evidence; wrapper only supervises

Teach `ingest_native_value` the current bounded event names for accurate liveness/diagnostics, including `compaction_start/end`, queue non-empty/count, `auto_retry_*`, and summarization retry states. Do not let raw stream text authorize a kick: raw bytes are observational and replayable, and the observer lacks an in-process delivery channel (`src/commands/pi_stream_bridge.rs:33-111,238-287`). Returned action vectors should either become durable outbox state or be removed from callers that cannot execute them; silent discard must end.

Keep wrapper responsibilities unchanged: launch gate, bootstrap, capture, wait/reap, accounting bridge, terminal status check (`src/commands/spawn/execution.rs:3500-3524,3665-3711`). It must not start a second Pi process for a post-compaction kick. A genuine dead process continues through existing convergence/reopen, which is explicitly a different attempt boundary (`src/commands/finalize.rs:1549-1702`).

### Why not RPC first?

RPC provides correlated prompts, `get_state`, `get_entries`, and a persistent stdin, so a future task-worker RPC transport could own delivery externally. The existing repository RPC implementation already proves the bidirectional mechanics and hermetic plugin argv (`src/commands/pi_handler.rs:340-444,497-539`). However it is a chat adapter, does not bind worker source/process authorization, and currently treats `agent_end` as completion. Replacing JSON capture/wrapper/accounting with RPC is materially broader than adding one authenticated plugin-to-watchdog outbox and does not eliminate the need for lifecycle first-terminal/budget checks.

## Citation-audit method

Current-implementation statements are separated from recommendations: the former carry file/function/line citations in their paragraph or table row; protocol proposals use “should”/“must” and are not represented as already-implemented behavior. Repository citations were mechanically checked for path existence and an in-range maximum line; that check does **not** prove semantic correctness. The semantic audit was the manual trace above, organized into explicit claim groups: launch/plugin loading, event projection, action consumption, marker delivery semantics, candidate-seam proofs, route-proof limits, terminal/wait suppression, budgets, and exit/convergence. External Pi API claims cite the host package documentation and are called out as host contracts rather than WG implementation. The eventual tests below are required because documentation/source inspection cannot prove a live Pi turn.

## Required tests for the eventual fix

The decisive regression must use the real generated wrapper with a fake Pi capable of loading the embedded extension (or a strict extension host), emit a threshold `compaction_start/end`, become settled with no terminal receipt, and prove a **second Pi turn in the same OS process, same session ID, same generation, same attempt ID/fence, same route, and same worktree**. Existing integration tests prove only state/action vectors (`tests/integration_pi_watchdog.rs:245-310`), while the exited-worker smoke proves the separate two-launch convergence fallback (`tests/smoke/scenarios/exited_worker_finish_convergence.sh:115-149`).

The regression should also prove:

1. `compaction_end(willRetry=true)` and non-empty Pi autonomous queues do not receive an extra kick;
2. duplicate/replayed compaction and plugin reload deliver the action once;
3. a `wg_done`, `wg_fail`, or `wg_wait` receipt racing authorization suppresses delivery by first-terminal/fence rules;
4. the finite epoch/elapsed budget holds and produces no source retry/breaker increment;
5. wrapper exit before acknowledgement falls into existing convergence, not a hidden third delivery path;
6. summary, queue text, provider errors, and stock prompt content do not leak into watchdog/UI projections;
7. direct Pi route configuration still loads the exact compatible embedded plugin, without relying on prior profile/setup state.

## Minimal likely production and test files

No implementation was made. The smallest credible change set is:

* `worksgood-pi/src/index.ts` plus a small new `worksgood-pi/src/continuation.ts` — Pi compaction/settled observation, authenticated claim, Pi-native delivery/reconciliation;
* `worksgood-pi/src/wg-backend.ts` — bounded internal claim/ack calls;
* `worksgood-pi/test/plugin.test.ts` and a new continuation unit test — registration, trigger, race, replay, and message-vs-entry semantics;
* `worksgood-pi/embedded/pi-worksgood/**` — regenerated version-locked build after plugin changes (the embed contract is enforced by `src/pi_plugin/mod.rs:45-55,675-709`);
* `src/pi_watchdog/mod.rs` — current event projection and durable kick outbox/receipts;
* `src/commands/pi_watchdog.rs`, `src/worker_control.rs`, `src/worker_cli.rs`, and `src/commands/service/ipc.rs` — capability-checked claim/ack operation using the existing worker-control path;
* `src/commands/spawn/execution.rs` (and possibly executor settings construction in `src/service/executor.rs`) — explicit hermetic task-worker plugin load and compat environment;
* `src/commands/pi_stream_bridge.rs` — stop silently discarding actionable results; keep it evidence-only under the chosen ownership;
* `tests/integration_pi_watchdog.rs` — projection, CAS, terminal/wait races, budget, and crash/replay;
* a new `tests/smoke/scenarios/pi_threshold_compaction_same_process_kick.sh` plus a grow-only `tests/smoke/manifest.toml` entry — generated-wrapper end-to-end proof.

`src/commands/pi_handler.rs` should not need production changes for the recommended plugin-owned JSON-worker delivery. It is a comparison/reference harness; changing it would indicate an RPC-worker redesign rather than the minimal fix.
