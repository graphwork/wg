# Design: authoritative WG Pi threshold-compaction kick

**Status:** proposed, implementation-ready; no code is implemented by this document
**Task:** `design-wg-pi-compaction-kick`
**Exact upstream defect:** [`earendil-works/pi#6424`](https://github.com/earendil-works/pi/issues/6424)
**Baseline:** installed `@earendil-works/pi-coding-agent` 0.83.0

Related evidence:

- [`research/pi-threshold-compaction-stall-reproducer.md`](research/pi-threshold-compaction-stall-reproducer.md)
- [`research/pi-compaction-recovery-patterns.md`](research/pi-compaction-recovery-patterns.md)
- [`research/wg-pi-compaction-continuation-seams.md`](research/wg-pi-compaction-continuation-seams.md)

## 1. Decision

Use a **split permit + in-process delivery protocol**:

- The Rust lifecycle kernel and `PiWatchdog` own whether a continuation is
  allowed, the durable action/outbox, the first-terminal race, exact source /
  process / session / route guards, and the existing finite continuation budget.
- The exact embedded `@worksgood/pi` extension owns only delivery into the live
  Pi session and acknowledgement. It may deliver only a permit returned through
  the attempt-scoped worker capability.
- The generated wrapper remains the launch/reap supervisor. The raw JSON
  observer remains a bounded evidence stream. Neither is a delivery channel.

The delivery point is the awaited `session_compact` extension callback for a
**successfully persisted threshold compaction**, while Pi is in its quiescent
post-agent continuation window. The extension enqueues one hidden custom
`followUp` before that callback returns. Pi's existing post-compaction queue
check then starts the next run before its first public `agent_settled` event.

This deliberately rejects the initially attractive “wait for
`agent_settled`, then call `sendUserMessage`” variant. In installed Pi 0.83.0,
`_emitAgentSettled()` sets the outer session idle and awaits extension handlers,
but `ExtensionAPI.sendMessage` and `sendUserMessage` return `void` to an
extension. A post-settled call starts a detached nested promise. JSON print mode
returns as soon as its original awaited `session.prompt()` completes and then
runs `disposeRuntime()` (`dist/modes/print-mode.js:93-129`). It does not await
that detached prompt. Such a design can emit the first `agent_settled`, abort or
lose the new run during disposal, and cannot acknowledge enqueue reliably. It
also fails the reproducer's required ordering.

At `session_compact`, Pi's public `ctx.isIdle()` is intentionally **false**:
`_isAgentRunActive` spans the post-run compaction/queue loop. That does not mean
a provider or tool is active. The extension must track the narrower state
`CompactionQuiescent = agent_end seen && no later agent_start && no open tool`,
require `ctx.isIdle() == false`, and enqueue with `deliverAs: "followUp"`.
Calling the same API in any other non-idle state is forbidden. Calling it after
`ctx.isIdle() == true` is also forbidden for one-shot JSON because it takes the
detached-prompt branch. This distinction is part of the supported Pi-host
contract and must be tested against the installed host.

Use `pi.sendMessage`, not a forged human `sendUserMessage`, for the actual kick:

```ts
pi.sendMessage(
  {
    customType: "wg-pi-compaction-kick",
    content: permit.prompt,
    display: false,
    details: {
      actionId: permit.actionId,
      promptVersion: permit.promptVersion,
      promptDigest: permit.promptDigest,
    },
  },
  { deliverAs: "followUp", triggerTurn: true },
);
```

Custom messages participate in LLM context, carry an unambiguous action ID for
acknowledgement/reconciliation, and are not represented as human input.
`triggerTurn` is harmless in the required active continuation window; the
`followUp` queue is what triggers the turn.

## 2. Safety properties and non-goals

The implementation MUST preserve these invariants:

1. **WG protocol, not prose, decides unresolved work.** A managed task is
   unresolved iff its exact lifecycle attempt remains running and no accepted
   `wg_done`, `wg_fail`, `wg_wait`/park, cancellation, abort, or other first
   terminal receipt exists. Neither assistant text nor the compaction summary is
   classified. “I am done” is not a WG receipt. **Normal final answer** has a
   protocol definition, not a prose definition: either (a) Pi follows the
   ordinary no-compaction control trace `agent_end(willRetry=false) ->
   agent_settled`, so no qualifying `session_compact` occurrence exists, or (b)
   a managed answer has its accepted terminal/park receipt before permit. Both
   are must-not-trigger cases. A final-sounding managed message followed by an
   actual qualifying threshold compaction but lacking the required WG receipt is
   not the normal control trace and remains lifecycle-unresolved. Suppressing it
   would require the forbidden prose heuristic and would reproduce #6424.
2. **Exactly-once durable authorization per occurrence; at-most-once
   delivery.** Every distinct qualifying persisted threshold-compaction entry
   gets exactly one durable occurrence/action record and epoch charge, with at
   most one fresh delivery grant/send invocation. On the no-crash path that
   grant produces exactly one kick. Duplicate/replayed events for that entry,
   daemon replies, and plugin reloads cannot create another fresh grant or Pi
   call. A second qualifying entry in the same attempt gets a different
   occurrence/action and, on the no-crash path, a second kick. Distributed
   guaranteed exactly-once delivery is explicitly **not** claimed: a crash after
   permit commit but before a proven reply/ack may yield zero observed delivery
   and is held indeterminate, never “fixed” by blind redelivery. Thus “exactly
   one kick is possible per occurrence” means one durable opportunity/maximum,
   not guaranteed liveness across the untransactional WG-to-Pi boundary.
3. **Finite shared authority, with no per-attempt kick cap.** Every kick consumes
   one existing Pi continuation epoch and its elapsed-time charge. There is no
   dedicated `kick_used`, one-kick-per-attempt flag, or compaction-kick count
   limit. Successive qualifying compactions continue to receive distinct kicks
   until the already-existing overall authorization limits
   (`max_continuation_epochs` / lifecycle `max_replacement_epochs` and the
   corresponding elapsed-seconds limit) are exhausted. Exhaustion is a loud
   `HeldOperatorRequired(continuation_budget_exhausted)`, not a silent stop and
   not permission to start a new route/session/process.
4. **Same owner.** At permit, the only owner/attempt accounting field changed
   by a kick is `pi_continuation_epoch`; action, effect-lease, ack, and abort
   receipts are bounded lifecycle bookkeeping. A kick does not change task
   generation, attempt ID/fence, attempt sequence, worktree path or lease epoch,
   Pi session ID/file/branch, process epoch, PID birth identity, or route
   snapshot.
5. **Fail closed.** Missing evidence, an old plugin/host, an uncertain send,
   queue races, terminal races, process exit, or any guard mismatch means no
   send. Existing exact-owner exit convergence remains the recovery path.
6. **No new effect authority.** The prompt is guidance. It confers no graph or
   tool capability beyond the worker's existing attempt capability.

This design does not change Pi's compaction policy, does not fix general human
Pi sessions, does not infer semantic incompletion, does not restart Pi, and does
not replace JSON workers with RPC.

## 3. Exact action identity

There are two related identifiers so lookup remains idempotent even after the
continuation epoch advances.

### 3.1 Durable compaction occurrence ID

The daemon, not JavaScript, locates the named entry in the attested session file
and hashes its exact canonical JSON. The Pi-managed compaction entry ID is the
durable unique event identity: a later compaction is a different descendant
entry with a different ID, while duplicate callbacks/raw lines refer to the
same entry. The daemon computes:

```text
occurrence_id = b3(canonical-json([
  "wg.pi.threshold-compaction-occurrence/v1",
  graph_id,
  task_id, generation, attempt_id, attempt_fence,
  worktree_lease_epoch,
  process_epoch, process_identity_digest,
  session_id, session_header_digest,
  compaction_entry_id, compaction_parent_id, compaction_entry_digest,
  route_snapshot_digest
]))
```

The occurrence tuple has a unique index in the watchdog ledger. A repeated
authorization request first looks it up and returns that record; it never
derives a fresh record from the now-advanced continuation epoch. A different
persisted compaction entry, including a later descendant in the same
session/process/attempt, is a different occurrence and MUST allocate a new
record. The record may also store a monotonically increasing
`occurrence_ordinal` per source/session for diagnostics, allocated in the same
insert transaction, but correctness and deduplication use `occurrence_id`, not
arrival order.

### 3.2 Exactly-once durable authorization/action key

On the first authorization only, the watchdog captures the then-current
continuation epoch and frozen stock prompt:

```text
action_id = b3(canonical-json([
  "wg.pi.threshold-compaction-kick/v1",
  occurrence_id,
  authorized_from_continuation_epoch,
  "WG_PI_COMPACTION_KICK_V1", prompt_digest
]))
```

The record fixes `to_continuation_epoch = from + 1`. The unique occurrence index
and action ID together make these illegal:

- two action IDs or fresh delivery grants for one compaction entry;
- one action applied to a different source/process/session/route;
- two epoch charges for a replayed permit;
- changed prompt bytes under an existing action ID.

They deliberately do **not** make “an attempt already kicked” illegal. If entry
`E1` produces `occurrence_id O1` / `action_id A1` and its recovery turn later
produces a second threshold-compaction entry `E2`, then `E2 != E1`, `O2 != O1`,
and `A2 != A1`. Subject to the shared overall epoch/time budget and all current
guards, `A2` receives its own permit and kick.

The prompt is the stock WG finalization instruction with a bounded observation
code such as `threshold_compaction_protocol_unresolved`. It MUST NOT include the
assistant's final prose, compaction summary, queue text, provider error, tool
arguments/output, or other untrusted content. Its version and digest are part of
the action.

## 4. Authority predicate

The embedded extension may ask; only the broker may authorize and permit. An
authorization succeeds only when every row below is proven from durable WG
state or exact local Pi state.

| Guard | Authoritative evidence | Failure result |
| --- | --- | --- |
| Managed unattended worker | Opaque `WG_WORKER_CAPABILITY` resolves to `WorkerOperationKind::PiCompactionKick`; binding has this graph/task/generation/attempt/fence/lease/agent | No registration/call in an ordinary human Pi process; reject a chat or stale capability |
| Current lifecycle | Task is `InProgress`; `current_attempt`, generation, fence, and lease equal the binding; `PiContinuationAuthorization.state == Active` | Cancel/hold |
| WG unresolved | No lifecycle `pi_terminal_reservation`, parked/waiting attempt, terminal disposition, cancel, abort, or superseding generation | Cancel; terminal is authoritative regardless of prose |
| Pi event | Exact embedded plugin reports `session_compact.reason == "threshold"` and `willRetry == false` | Ignore manual and overflow |
| Successful persistence | The event supplies a compaction entry ID; broker reconciles the selected session journal, finds exactly one matching current leaf, verifies its parent and digest | Reject missing/ambiguous/old/forked entry. Failed/aborted compaction has no `session_compact` and cannot qualify |
| Quiescent host window | Plugin saw `agent_end`, no later `agent_start`, no open tool, `ctx.isIdle() == false`, and this is the awaited `session_compact` callback | Reject generic non-idle/provider/tool activity and reject post-settled `ctx.isIdle() == true` delivery |
| Empty native queues | `ctx.hasPendingMessages() == false` before authorize, before permit, and immediately before send; embedded handler is loaded last | Cancel if steering/follow-up is queued; never compete with real continuation |
| Effect safety | Watchdog has no open `ToolContract`; `exact_guards.effect` is true; raw-stream projection has matched starts/ends | Hold on unsafe or indeterminate in-flight effects |
| Exact session | Session ID, selected file/header, current compaction leaf, append-only prefix, and no branch/fork replacement match `SessionProof` | Hold/cancel |
| Exact route | Handler/provider/model/reasoning and route digest match bootstrap; plugin compat/artifact digest matches; no intervening `model_select` | Hold/cancel. Never “resume” on a newly resolved profile route |
| Exact process | Process epoch and PID/PGID/start ticks/boot ID/nonce match. Broker verifies the `wg` claimant descends from the exact Pi child | Hold/cancel |
| Budget | Existing overall watchdog/lifecycle continuation epoch and elapsed limits admit `from + 1`; there is no dedicated kick count | Loud `HeldOperatorRequired(continuation_budget_exhausted)`; no permit |
| Feature/host contract | Kill switch enabled; exact compatible embedded plugin loaded hermetically; supported host exposes awaited `session_compact`, custom message lifecycle, and active-window `followUp` behavior | No action; loud diagnostic |

The broker should reconcile bounded raw-stream observations to the current
complete line before evaluating effect/process evidence. Raw events never make
the unresolved-work decision and never create an action by themselves.

### Queue-race boundary

Pi exposes no transactional “queue is empty and append this message” method, so
WG MUST establish an equivalent **host-serialized critical section**, not rely
on timing. For the pinned supported host, both `ctx.hasPendingMessages()` and
`pi.sendMessage()` are synchronous, extension handlers execute in one Node
isolate, and Pi awaits the current `session_compact` handler before core queue
processing resumes. Installed 0.83.0 supplies concrete proof points:
`dist/core/extensions/runner.js:576-585` iterates and awaits handlers in load
order, `dist/core/agent-session.js:1674-1709` awaits `session_compact` before its
queue decision, and `dist/core/extensions/types.d.ts:231-240` plus
`docs/extensions.md:1388-1410` expose synchronous queue/read send signatures.
After the asynchronous permit call returns, the final
handler code MUST execute these two statements in one JavaScript call stack,
with no `await`, promise continuation, timer, `queueMicrotask`, callback, or
other yield between them:

```ts
if (ctx.hasPendingMessages()) return suppressAfterPermit(actionId, "queue_nonempty");
pi.sendMessage(message, { deliverAs: "followUp", triggerTurn: true });
```

JavaScript run-to-completion is the linearization boundary: no extension,
native-queue callback, or microtask can mutate the queue between the read and
the synchronous append. The hermetic launch makes that claim enforceable:
disable ambient extension/settings discovery, explicitly load the embedded
plugin last, give one-shot JSON stdin EOF after the initial prompt, and reject
any explicit extension allowed to retain a background queue writer. Earlier
handlers (including the credential-free provider) are awaited before the final
handler, so work queued before this critical section is visible. A startup
host-contract probe and the real-host smoke MUST prove sequential handler
awaiting, synchronous queue read/append, and same-stack ordering; an unknown
host, failed probe, asynchronous wrapper, or possible out-of-isolate writer
disables the feature loudly. A message observed after authorization but before
the critical section suppresses the action. Once the same-stack append occurs,
there was no intervening queued-message race.

## 5. Protocol

Add capability-scoped action operations plus an effect/cancellation interlock.
Suggested internal CLI spelling for one-shot operations is:

```text
wg pi-watchdog compaction-kick authorize ...
wg pi-watchdog compaction-kick permit --action <id> ...
wg pi-watchdog compaction-kick ack --action <id> ...
wg pi-watchdog compaction-kick cancel --action <id> --reason <code>
wg pi-watchdog compaction-kick effect-begin --action <id> --tool-call <id>
wg pi-watchdog compaction-kick effect-end --action <id> --tool-call <id>
wg pi-watchdog compaction-kick abort-ack --action <id> --terminal <receipt-id>
```

The plugin also opens an action-scoped terminal-cancellation subscription over
the existing local worker-control IPC before sending. These are not operator
continuation commands. In worker mode they translate to typed
`WorkerOperation`s and cannot fall back to graph-file access.

### 5.1 Observe and authorize

The plugin's awaited `session_compact` handler performs its local guards and
calls `authorize` with only bounded identity fields: event reason/retry bit,
session ID, compaction entry ID/parent ID, current Pi PID, model identity,
plugin compat, and a deterministic request ID based on the entry ID. It sends no
summary or prompt text.

The broker authenticates the capability and exact process ancestry, reconciles
the session entry from disk, evaluates §4, and persists an `Authorized` watchdog
outbox record before replying. Authorization does not increment an epoch and is
safe to replay. A duplicate occurrence with identical fields returns the same
action; the same occurrence ID with different fields is a conflict. A later
entry ID is not a duplicate merely because task, attempt, process, or session is
the same.

### 5.2 Delivery permit — the linearization point

After authorization returns, the plugin repeats its local session/phase/tool/
queue guards and calls `permit(action_id)`. In one graph modification guarded by
`FenceExpectation::current`, the broker rechecks current attempt, exact process,
terminal-clear state, active continuation authorization, and budget, then uses
the existing `PiContinuationEpochReserved` transition to CAS
`from_continuation_epoch -> to_continuation_epoch`. The transition request's
idempotency key is the action ID.

This lifecycle CAS is the **linearization point** against `wg_done`, `wg_fail`,
`wg_wait`, cancel, and abort. The broker then persists the watchdog record as
`DeliveryPermitted` and only the request that performed that successful CAS may
receive `{ freshDeliveryGrant: true, prompt, permitReceipt }`. A request that
finds the action already `DeliveryPermitted` receives status only,
`freshDeliveryGrant: false`, and no prompt authority. Concurrent duplicate
permit calls therefore cannot both send.

If the graph commit succeeded but the watchdog write or reply crashed, replay
finds the lifecycle audit event with this action ID and exact epoch and repairs
the outbox to `DeliveryPermitted`, but returns a **non-deliverable** replay
status. WG cannot prove whether the original reply crossed the process boundary,
so liveness is sacrificed rather than minting a second send opportunity. If the
broker crashes before the graph CAS, the record remains `Authorized`; replay of
the deterministic request may perform the one CAS and receive the fresh grant.
No path charges the epoch twice.

A terminal/park that commits first rejects the permit. A permit that commits
first authorizes at most this one delivery; a later terminal cannot rewrite
history, but §5.4 makes an accepted terminal revoke the run, block all later
effects, and request immediate Pi abort. The acknowledgement response also tells
the plugin to abort if a terminal/park won after permit but before ack. Absent a
terminal, the same attempt remains eligible for a later, distinct compaction
occurrence under the shared continuation budget.

### 5.3 Enqueue and acknowledge

Immediately after receiving a **fresh** delivery grant, with no timer or
event-loop deferral, the plugin checks the same local identity/queue/quiescence
tuple once more, atomically marks that action ID `sendAttempted` in its live
module before invoking Pi, and calls
`pi.sendMessage(..., { deliverAs: "followUp", triggerTurn: true })` exactly
once. Duplicate callbacks and permit responses consult that live set and do not
call Pi. The action remains `DeliveryPermitted` during the unavoidable send/ack
gap; this state means “send or a deliverable reply may have happened,” never
“safe to retry.” A plugin reload loses the live set but cannot obtain a fresh
grant, so it still cannot resend.

The plugin registers `message_start`. This is a substantiated dequeue event on
the pinned host, not an assumed callback: `pi-agent-core/dist/agent-loop.js:95-
101` emits `message_start`/`message_end` for each pending follow-up before adding
it to current context; `dist/core/agent-session.js:350-365` awaits forwarding
that exact message to extension handlers and persists a custom message on end;
and `dist/core/agent-session.js:1065-1084` shows queued custom messages preserve
`customType`, content, and `details`. When the handler sees the exact custom
message whose `customType`, action ID, prompt version, prompt digest, and content
digest match the permit, it calls `ack` before returning. This proves that the
exact live Pi process selected the permitted message for agent context. `ack`
rechecks source/process/session/action, persists `Acknowledged`, and is
idempotent. The following `message_end`/Pi-managed `custom_message` session entry
is the durable reconciliation proof if the ack RPC response is lost. A later
raw `agent_start`/turn and terminal receipt are ordinary watchdog/lifecycle
evidence, not additional authority.

If ack fails transiently after message selection, the plugin may retry **ack
only** on later matching message/turn events. It may never call `sendMessage`
again. On session reload it may acknowledge an already persisted matching Pi
custom-message entry, but it may not reconstruct or send an action from a
historical compaction entry.

The next public `agent_settled` after an acknowledged kick closes the latest
open action as `SettledAfterKick`. If WG remains unresolved, the watchdog holds
for existing process-exit convergence/operator handling; it MUST NOT manufacture
another kick from settlement alone. Before settlement, however, the recovery
turn may itself finish with another successful qualifying threshold compaction.
That new persisted entry starts a new occurrence/action and receives its own
kick if the existing shared overall epoch/time budget still admits it. There is
no one-kick-per-attempt suppression.

### 5.4 Accepted-terminal cancellation and effect interlock

Acknowledgement means Pi selected the kick; it does not give the recovery run a
right to outlive a later accepted `wg_done`, `wg_fail`, or `wg_wait`. Before the
fresh send, the plugin opens an action-scoped terminal subscription over the
existing daemon IPC and installs the embedded plugin as the final `tool_call`
handler. The subscription is advisory for prompt/token cancellation; the
authoritative no-later-effect property is a lifecycle CAS interlock:

1. Every recovery-run tool other than the dedicated WG terminal tools is
   conservatively effectful. Installed Pi explicitly defines `tool_call` as
   occurring before execution and as block-capable
   (`docs/extensions.md:751-765`). Hermetic launch permits no earlier effectful
   `tool_call` handler; any such extension disables kicking. The final handler
   calls `effect-begin(action_id, toolCallId)` and returns `{ block: true }`
   unless the
   lifecycle atomically proves the exact running action/process/session, no
   terminal receipt, and opens an idempotent `ToolContract` effect lease. Only
   then may Pi execute the tool. `tool_result`/`tool_execution_end` closes that
   exact lease with `effect-end`; ambiguous/crashed leases remain unsafe/held.
2. `PiTerminalIntent` and `AttemptParked` use the same lifecycle serialization.
   They may become **accepted** only when no kick effect lease is open. If one is
   open, the terminal request is pending/refused with `effect_in_flight` and no
   accepted terminal receipt is created; it may retry after the exact effect
   end. The terminal CAS sets the receipt, revokes continuation/action authority,
   and makes every later `effect-begin` fail in the same transaction.
3. The WG terminal tools (`wg_done`, `wg_fail`, `wg_wait`) do not open a normal
   effect lease. Their preflight requires no sibling lease; their execution
   performs the terminal/park CAS. A terminal tool preflighted alongside an
   already-leased parallel sibling is blocked and must be retried alone, so it
   cannot accept terminal while that sibling executes. A shell command that
   invokes `wg done` remains under its shell lease and therefore cannot create
   an accepted terminal receipt mid-effect.
4. On accepted terminal, the daemon publishes its receipt on the subscription.
   The plugin calls `ctx.abort()` against the active run and sends idempotent
   `abort-ack`. If notification or abort acknowledgement is lost, lifecycle
   revocation plus the final `tool_call` gate still proves that **no new effect
   can start**; only provider text may drain until reconnect, settlement, or
   process reap. There was no effect already running at terminal commit by rule
   2. Reconnect may retry abort/ack, never the kick.

This interlock narrows existing worker authority; it grants no new effect
capability. Process exit with an ambiguous open lease uses the existing unsafe-
effect/exit reconciliation and is held loudly rather than fabricated closed.
The implementation may represent leases by extending the existing
`ToolContract` projection, but the terminal check and lease count/action binding
must live in lifecycle state so effect-begin versus terminal is one CAS domain,
not a race between the graph and watchdog files.

## 6. Durable state machine

`PiCompactionKickRecord` is a bounded map/list in `PiWatchdogState`, keyed by
occurrence and action IDs. Terminal states are retained for the source attempt.
The bound is the existing overall continuation-epoch budget plus small
suppressed-diagnostic retention; it is not a one-action slot. State transitions
below are per record, so creating `O2/A2` never reopens or mutates `O1/A1`.

| Current state | Input / condition | Transaction and next state | External action |
| --- | --- | --- | --- |
| `Absent` | `compaction_start` raw JSON | Record bounded diagnostic reason/start sequence only | None |
| `Absent` | `session_compact(threshold, willRetry=false)` and all authorize guards pass | Persist immutable record as `Authorized` | Return action metadata, no prompt authority yet |
| `Absent` | manual, overflow, pending queue, non-quiescent host, terminal WG state, unsafe effect, mismatch, or disabled | No action, or bounded `Suppressed(reason)` diagnostic | None |
| `Absent` | otherwise qualifying occurrence but shared overall budget exhausted | Persist/emit loud `HeldOperatorRequired(continuation_budget_exhausted)` diagnostic | None; never silently loop or restart |
| `Absent` | failed/aborted `compaction_end` | Record diagnostic outcome; no qualifying occurrence exists | None |
| `Authorized` | identical duplicate authorize | No mutation | Return same action |
| `Authorized` | changed payload for same occurrence | `HeldConflict` | None |
| `Authorized` | local guard fails / queue appears | `Cancelled(reason)` | None; budget was not charged |
| `Authorized` | permit and terminal-clear CAS succeeds | Lifecycle epoch increments once; persist `DeliveryPermitted` | Return frozen prompt/permit with `freshDeliveryGrant=true` only to this winning call |
| `Authorized` | terminal/park/process/mismatch wins | `Cancelled` or `HeldMismatch` | None |
| `Authorized` | shared overall epoch/time budget is now exhausted | `HeldOperatorRequired(continuation_budget_exhausted)` | None; loud status/diagnostic |
| `DeliveryPermitted` | duplicate/recovered permit request | No increment | Return `freshDeliveryGrant=false`; never return prompt/send authority again |
| `DeliveryPermitted` | final local queue/quiescence/tool guard fails before Pi API call | `DeliverySuppressedAfterPermit(reason)`; epoch remains charged | No send and no replacement grant |
| `DeliveryPermitted` | plugin invokes Pi API | State remains `DeliveryPermitted` (indeterminate until ack) | Exactly one local send call |
| `DeliveryPermitted` | exact custom `message_start`, terminal still clear | Persist `Acknowledged` | Pi continues existing process/session |
| `DeliveryPermitted` | exact custom `message_start`, terminal/park won after permit | Persist `AcknowledgedTerminalRace`; reply `abort=true` | Plugin calls `ctx.abort()`, never resends |
| `DeliveryPermitted` | process exits or session/process identity changes before ack | `Uncertain` | No redelivery; existing exit convergence |
| `Acknowledged` | duplicate ack/replayed message event | No mutation | None |
| `Acknowledged` | later `agent_start`/turn evidence | Optional `Running` diagnostic; same action | None |
| `Acknowledged` / `Running` | final `tool_call` gate wins terminal-clear CAS | Open action/tool-call-bound effect lease; state remains running | Permit exactly that tool execution |
| `Acknowledged` / `Running` with open effect lease | terminal/park requested | No terminal transition/receipt; return pending/refused `effect_in_flight` | Existing effect finishes or remains loudly held |
| `Acknowledged` / `Running` with zero effect leases | terminal/park CAS succeeds | `TerminalObserved`; atomically revoke action/continuation and future effect-begins | Publish cancellation; plugin calls `ctx.abort()` |
| `TerminalObserved` | matching abort acknowledgement | `TerminalAbortAcknowledged` | Normal finalization/reap; no effect/kick authority |
| Any existing record(s) | a **different** persisted threshold-compaction entry qualifies while source remains running | Insert new `occurrence_id`/`action_id` record as `Authorized`; prior records unchanged | Start the independent permit flow for this occurrence |
| latest `Acknowledged` / `Running` | later `agent_settled`, no terminal or newer occurrence | `SettledAfterKick`; hold `kick_completed_without_terminal` | No settlement-derived prompt/kick; only a future distinct compaction event could authorize |
| `Running` with open effect lease | tool-end missing or exact process exits | `HeldUnsafeEffect` until existing receipt/exit reconciliation proves disposition | Never infer success or grant another kick/effect |
| Any other nonterminal action | exact process exit | `Authorized -> CancelledProcessExit`; `DeliveryPermitted -> Uncertain`; acked states retained | Wrapper/lifecycle exit path only |
| Any | duplicate raw/plugin lines, callback replay, or finished-stream replay for the same entry | Cursor/occurrence/action/request IDs make transition idempotent | Raw observer never sends; broker never issues another fresh grant |

`compaction_end` is still projected. A matching successful threshold end
confirms diagnostics. A missing end because the process died leaves the action
cancelled/uncertain according to its state. A contradictory end after a
`session_compact` (failure, abort, changed reason/retry) is a host-contract
violation: mark `HeldMismatch`, prevent all later permits, and surface it
loudly. It cannot authorize a retry.

## 7. Suppression table

| Must not over-trigger | Explicit rule |
| --- | --- |
| Normal final answer | Exact no-compaction control trace (`agent_end(willRetry=false) -> agent_settled`) creates no occurrence/action at all. If threshold compaction did occur, an accepted terminal/park receipt wins before permit. Prose is ignored. An unmanaged final has no capability/handler. “Final-sounding + threshold occurrence + no WG receipt” is explicitly not this control case because no receipt-safe classifier can call it complete |
| Manual compact | `event.reason != "threshold"`; ignore even when successful and idle |
| Overflow | `willRetry == true` (and/or reason `overflow`); Pi already owns its one retry |
| Failed compaction | No successful `session_compact` entry; `compaction_end.result` absent/error |
| Aborted compaction | No successful qualifying entry; `aborted == true` diagnostic only |
| Queued steering/follow-up | `ctx.hasPendingMessages()` before authorize/permit or in the final same-stack queue-read/send critical section cancels/suppresses; raw queue counts corroborate only. Unsupported host serialization disables the feature |
| Non-idle provider/tool run | Require the post-`agent_end`, pre-settlement `CompactionQuiescent` automaton and no open tool. A random `session_compact`/reload/timer cannot send |
| Already idle/settled JSON | `ctx.isIdle() == true` is rejected because it would start a detached prompt that print mode does not await |
| `wg_done` / `wg_fail` / `wg_wait` | Lifecycle first-terminal/park CAS before permit cancels. After permit/ack, terminal may be accepted only at zero effect leases; it atomically revokes later effect/kick authority, publishes cancellation, and the plugin aborts/acks. A terminal racing an open effect is not yet accepted |
| Unsafe in-flight effect | Open/unsafe/receipt-ambiguous `ToolContract`, effect lease, or false effect guard holds the action and terminal acceptance; every recovery tool passes the final lifecycle effect gate |
| Route mismatch | Frozen route digest/model/reasoning/plugin artifact must match; profiles/config are not re-resolved |
| Session/branch mismatch | Exact ID/header/file/current compaction leaf/prefix must match; no fork/resume selector substitution |
| Process mismatch/exit | Exact process epoch and PID birth identity must match; no replacement process is launched |
| Exhausted continuation budget | Only the existing shared overall epoch/elapsed limits apply. Exhaustion records/surfaces `HeldOperatorRequired(continuation_budget_exhausted)`; there is no per-attempt kick-used cap |
| Duplicate/replayed event | Occurrence/action unique keys return the existing state and never a second fresh grant; a distinct later compaction entry is not a duplicate |
| Non-WG human/chat Pi | Missing attempt capability or chat binding: continuation module is inert and performs no broker call |
| Historical compaction on upgrade/reload | Never backfill; only a live awaited qualifying callback may authorize |

## 8. Crash and race matrix

The table uses `A` = durable `Authorized`, `P` = lifecycle CAS committed and
`DeliveryPermitted`, `S` = Pi API send invoked, and `D` = durable delivery ack.

| Crash/race point | Durable evidence after recovery | Replay behavior | Outcome |
| --- | --- | --- | --- |
| Normal final-answer control: `agent_end(willRetry=false) -> agent_settled` with no `session_compact` | No occurrence/action | Settlement alone never authorizes | No kick, regardless of final prose bytes |
| Before `session_compact` / during `compaction_start` | No action; optional bounded start diagnostic | Do not infer from start | Process exit/convergence or Pi's normal behavior |
| Compaction fails/aborts before saved entry | No action | Replayed end remains diagnostic | No kick |
| Saved entry, crash before `A` | Session may contain compaction; no live authorization record | Do **not** backfill from session | No kick; process-exit convergence |
| After `A`, before permit request | `Authorized`, epoch unchanged | Same live process/callback may request permit once; a restarted/different process cancels stale `A` | Safe retry of authorization/permit, no send yet |
| Terminal/park before permit CAS | Terminal receipt; `A` may exist | Permit rejects and marks cancelled | Terminal wins, no send |
| During permit before graph commit | `A`, old epoch | Replay retries same CAS/action ID | At most one epoch charge |
| Graph permit commit, crash before watchdog `P`/reply | Lifecycle audit has action ID and new epoch; watchdog may still have `A` | Reconcile to `P`, but any replay returns `freshDeliveryGrant=false`; never reconstruct a deliverable reply | Permit is not charged twice; this occurrence may lose liveness, never duplicate |
| Message queues while permit RPC is awaited | `P` or `A`; final local queue read sees nonempty | Suppress/cancel according to whether permit committed; never append kick | Real queued work wins |
| Final queue read versus send | Same synchronous handler call stack on a host that passed the serialization probe | No callback/microtask can interleave; append is the next statement. If this cannot be established, feature is disabled before authorization | No check/send race |
| After permit reply, final local guard fails before `S` | `P`, plus plugin cancel/status reason when reachable | Mark `DeliverySuppressedAfterPermit`; keep the epoch charge and never grant again | Queued real work or unsafe phase wins; zero kick for this occurrence, no duplicate |
| After permit reply, before `S` (crash) | `P`; send unknown | Never automatically resend. If the original handler is still executing it may make its one immediate call; after restart/exit mark uncertain | At-most-once beats liveness |
| During/after `S`, before matching `message_start` | `P`; Pi queue may contain action | Do not send again. Original process may drain and ack; reload may only inspect/ack a persisted matching custom entry | No duplicate; uncertain on exit |
| Matching message selected, crash during ack RPC | Pi observed exact action; watchdog may still show `P` | Retry ack only from matching events/session entry; never send | Eventually ack or remain uncertain |
| After `D`, before provider/assistant response | `Acknowledged` | Duplicate authorize/permit/ack are no-ops | Never redeliver; process exit uses convergence |
| Terminal commits after `P`, before `D` | Permit and terminal both ordered; no effect lease exists yet | Matching ack returns `abort=true`; cancellation subscription also fires | Plugin aborts; one committed kick maximum for this occurrence |
| `effect-begin` races terminal after `D` | One lifecycle CAS order | Terminal first rejects/blocks the tool; effect lease first makes terminal pending/not accepted until exact `effect-end` | No accepted terminal overlaps an effect |
| Terminal requested after `D` with an effect lease open | Running action + exact open lease; no terminal receipt | Return/persist pending `effect_in_flight`; after close, retry terminal CAS | Existing effect is not mislabeled post-terminal; no accepted terminal yet |
| Terminal commits after `D` with zero effect leases | Terminal receipt + revoked action; cancel notification may be unacked | Every later effect-begin fails; plugin calls/retries `ctx.abort()` and abort-ack | Accepted terminal suppresses all later effects even if provider text drains |
| Crash/loss after terminal commit before cancel/abort ack | Durable terminal/revocation; zero effects at commit | Reconnect repeats cancel; final tool gate remains fail-closed; process reap is fallback | No later effects; no kick replay |
| Public `agent_settled` before a kick | This means delivery did not enter the required active callback | Do not send post-settled; hold and let wrapper exit | Avoid JSON detached-run loss |
| Public `agent_settled` after `D` | Latest action becomes `SettledAfterKick` | No settlement-derived second prompt | Hold/operator/convergence; settlement is not a compaction occurrence |
| Process exits with `A` only | `CancelledProcessExit` | No permit on a new PID | Existing new-attempt convergence |
| Process exits with `P` but no `D` | `Uncertain` | Never redeliver same action | Existing new-attempt convergence |
| Process exits after `D` unresolved, no open effect | Delivered action plus exact reap | No same-action retry; convergence may create a **new generation/attempt** under existing rules | Ownership boundaries stay explicit |
| Process exits with an ambiguous open effect lease | Lease + exact reap but no end receipt | Existing unsafe-effect reconciliation; loudly hold rather than close/retry | No hidden continuation or terminal fabrication |
| Duplicate raw/plugin events for one compaction entry | Same stream cursor/occurrence/action/request IDs | Return stored state or conflict on changed bytes; no fresh grant | No extra send/charge |
| Second qualifying threshold compaction after kick 1 | A different persisted descendant entry `E2`, with `O2/A2`; `A1` is already acked and shared budget remains | Insert/permit `A2` independently; duplicate `E1` or `E2` events still resolve to their own existing records | Exactly two no-crash kicks and two epoch charges, one per occurrence |
| Daemon restarts | Watchdog outbox + lifecycle audit survive | Reconcile `A`/`P` ordering before replying | Same action only |
| Plugin reload, same process | Session scan may find a persisted custom action | Ack existing action only; never authorize from historical compaction | No replay send |

This is exactly-once for durable occurrence/action creation and epoch charging,
but intentionally at-most-once at the Pi API boundary with an explicit
`Uncertain` state. Claiming guaranteed exactly-once delivery across the `P -> S
-> D` crash window would be false because Pi exposes neither a transactional
WG permit + Pi queue commit nor a send promise/receipt to extensions. The
no-crash conformance path requires one observed kick per permitted occurrence;
crash paths require zero-or-one, never two.

## 9. Preservation and accounting

A successful permit records these before/after assertions:

| Field | Before | After kick |
| --- | --- | --- |
| task generation | `g` | `g` |
| attempt ID / fence / sequence | `a / f / n` | unchanged |
| worktree path / lease epoch | `w / l` | unchanged |
| Pi session ID / file | `s / file` | unchanged |
| Pi branch | compaction entry is current leaf | Pi session manager appends the custom message and responses as descendants; no fork/new session |
| process epoch / PID identity | `p / pid-digest` | unchanged when same PID |
| route snapshot | `route_digest` | unchanged; no profile re-resolution or model switch |
| continuation epoch | `c` | `c + 1` exactly once for this occurrence at permit (`c + 2` after two separately permitted occurrences) |
| kick effect leases | none | transient action/tool-call-bound receipts only; zero before accepted terminal and after settled reconciliation |
| retry/admission/breaker/eval/accounting domains | current values | no increments caused by the kick (ordinary Pi usage still accounts normally) |

The raw observer must project every recovery run's ordinary usage once. Each
new qualifying occurrence repeats only the continuation-epoch row above; the
kick itself adds no source retry or replacement-process accounting. There is no
attempt-wide boolean whose value changes after the first occurrence.

## 10. Native event projection

Extend `PiWatchdog::ingest_native_value` with bounded, content-free projections:

- `compaction_start`: reason enum and event sequence;
- `compaction_end`: reason, `aborted`, `willRetry`, success/error boolean, and a
  stable result/entry digest when available—not summary/error text;
- `queue_update`: steering/follow-up counts or nonempty bits only;
- current `auto_retry_start/end` and
  `summarization_retry_scheduled/attempt_start/finished` names;
- matching kick custom-message action ID digest and second-run boundaries.

Persist the stream cursor even when a recognized event changes only projection
state. Duplicate and finished replay remain byte-offset-idempotent. The raw
observer must not call authorize/permit/send. Remove or stop returning
`LaunchSameSession` / `AppendCompletionPrompt` from production paths that cannot
execute them; in particular, an `agent_settled` after a kick must not reserve a
new generic prompt epoch. Direct session-file appends remain evidence only and
must not be used for the new context-bearing message.

## 11. Production file plan

Current-code anchors for the seam are `PiWatchdogState` and its exact proofs at
`src/pi_watchdog/mod.rs:440-493`, the incomplete native event match at
`src/pi_watchdog/mod.rs:988-1081`, the current non-delivering prompt marker path
at `src/pi_watchdog/mod.rs:1676-1781`, and the lifecycle continuation/terminal
CAS at `src/lifecycle.rs:1079-1220`. Production currently discards returned
actions in `src/commands/pi_stream_bridge.rs:70-91,176-190` and
`src/commands/pi_watchdog.rs:947-961`. The real JSON wrapper is generated at
`src/commands/spawn/execution.rs:3500-3524,3665-3711`, while the plugin currently
wires no continuation handler at `worksgood-pi/src/index.ts:75-89`.

### Rust authority, projection, and broker

- `src/pi_watchdog/mod.rs`
  - add schema-v3 bounded compaction/queue/retry projection;
  - add `PiCompactionKickRecord`, states, occurrence/action derivation,
    authorize/permit/ack/cancel/reconcile methods;
  - reuse continuation budget and exact guards;
  - suppress the generic settlement prompt after an acknowledged kick;
  - never append the kick directly to the Pi JSONL file.
- `src/commands/pi_watchdog.rs`
  - add capability-only authorize/permit/ack handlers;
  - reconcile exact journal/process/route/worktree evidence;
  - use the existing lifecycle `PiContinuationEpochReserved` CAS with action ID
    as transition idempotency key;
  - reconcile the graph-commit/watchdog-write crash split.
- `src/lifecycle.rs`
  - no new retry domain; preserve first-terminal semantics and reuse
    `PiContinuationEpochReserved`;
  - add the action ID to epoch reservation and lifecycle-owned, idempotent
    `PiKickEffectLeaseOpened/Closed` records keyed by action/tool-call/process;
  - make `PiTerminalIntent` and `AttemptParked` require zero open kick effect
    leases, then atomically revoke the action/continuation so later effect-begin
    fails. This is the terminal/effect CAS domain, not parallel authority;
  - bump `LIFECYCLE_SCHEMA_VERSION` from 1 to 2 with serde-default fields.
- `src/worker_control.rs`, `src/worker_cli.rs`,
  `src/commands/service/ipc.rs`, and `src/cli.rs`
  - add typed authorize/permit/ack/cancel, effect-begin/end, terminal-watch, and
    abort-ack payloads;
  - add a dedicated `WorkerOperationKind::PiCompactionKick` to newly issued
    capabilities; do not silently broaden already issued capabilities;
  - use deterministic request IDs; make terminal-watch a bounded local IPC
    subscription whose reconnect replays only the durable terminal receipt.
- `src/commands/pi_stream_bridge.rs`
  - project current Pi names and action evidence; remain evidence-only;
  - eliminate silent “action vectors imply delivery” behavior.
- `src/commands/spawn/execution.rs` and, if needed,
  `src/service/executor.rs`
  - call `ensure_pi_plugin(EnsureMode::Hermetic)` for **task** Pi workers;
  - pass `-e <exact embedded>/pi-worksgood/index.js` explicitly and last, disable
    ambient discovery (`-ne`/equivalent), inject compat plus exact session/run
    identity, and freeze the route;
  - preserve the current generated wrapper, JSON capture, child PID bootstrap,
    and reap flow.

`src/commands/pi_handler.rs` is not part of the production change. It remains a
reference for hermetic plugin materialization; this task does not migrate task
workers to RPC.

### Embedded plugin

- new `worksgood-pi/src/continuation.ts`
  - finite event automaton (`agent_start/end`, tools,
    `session_compact`, `message_start`, `agent_settled`, shutdown);
  - local guard checks, same-stack queue-read/send, authorize/permit/send-once
    per occurrence, independent successive occurrences, and ack-only replay;
  - final `tool_call` effect-begin gate, effect-end receipts, terminal
    subscription, `ctx.abort()`, and abort acknowledgement;
  - no registration without the exact worker capability/task-worker launch
    contract or a passing pinned-host serialization probe.
- `worksgood-pi/src/index.ts`
  - install the continuation module after tools/commands/model bridge so it is
    the final embedded handler.
- `worksgood-pi/src/wg-backend.ts`
  - retain the opaque worker capability/session/process/route launch fields and
    add typed JSON helpers for the broker operations.
- `worksgood-pi/src/version.ts`, `src/pi_plugin/mod.rs`, and
  `worksgood-pi/embedded/**`
  - bump plugin compatibility and regenerate with
    `make embed-worksgood-pi`.
- `worksgood-pi/test/plugin.test.ts` plus a focused new continuation test
  - registration, quiescent-window qualification, same-stack queue/send proof,
    unsupported-host disable, queue/terminal/mismatch suppression, send once per
    distinct occurrence (including two in one attempt), duplicate-event
    deduplication, effect/terminal CAS outcomes, abort notification/ack, ack-only
    replay, and host-contract assertions.

### Tests

- `tests/integration_pi_watchdog.rs`: state transitions, exact occurrence/action
  key, two successive occurrence records, per-occurrence deduplication,
  lifecycle CAS, terminal/wait versus effect-begin/end races, post-ack terminal
  revocation/abort, effect/session/route/process guards, shared budget, and all
  crash barriers.
- existing `tests/fixtures/fake-pi-compaction-stall/**`: retain the upstream
  defect/control reproducer; extend or add a companion wrapper fixture without
  turning it into the authority.
- new
  `tests/smoke/scenarios/pi_threshold_compaction_same_process_kick.sh` and a
  grow-only `tests/smoke/manifest.toml` entry owned by
  `implement-wg-pi-compaction-kick`.

## 12. Schema, compatibility, migration, and rollback

### Schema

Bump `PiWatchdogState.schema_version` from 2 to 3. New action-ledger and native
projection fields use `#[serde(default)]`. Schema-2 migration initializes an
empty ledger and current bounded projection. It MUST NOT scan old Pi sessions
or raw streams to create actions.

Reuse lifecycle's existing continuation authorization and epoch transition,
but bump `LIFECYCLE_SCHEMA_VERSION` from 1 to 2 for the serde-defaulted action
ID, kick effect-lease map, and terminal-revocation/abort status. V1 migration
starts with no kick action and no effect leases. It does not make an old active
attempt eligible: only a newly issued capability plus matching plugin can open a
lease or kick, so treating old state as empty cannot race an old recovery run.
There is no task graph status migration and no new task generation/attempt.

The typed worker operations are additive. Keep
`WORKER_CONTROL_PROTOCOL = worksgood-worker-control-v2` if old peers already
fail closed on unknown operation tags and cannot subscribe; otherwise bump it
once and migrate daemon/CLI/plugin together. In either case, old capabilities
lacking the dedicated operation cannot kick, open an effect lease, or subscribe,
and must wait for a newly spawned attempt.

### Plugin/host compatibility

Bump `WG_PI_PLUGIN_COMPAT_VERSION` (currently `0.2.0`) because the new embedded
plugin depends on new broker operations and exact Pi lifecycle semantics. The
Rust binary and embedded `version.ts`/`version.json` remain lock-step. Startup
must fail loudly on mismatch. Add an explicit supported-host self-test for the
installed Pi behavior used here; an open peer dependency is not sufficient.

### Rollout and rollback

- Default **on only for hermetic unattended Pi task workers** newly spawned by a
  matching binary/plugin. Human Pi, chats, ambient/discovery-based workers, and
  existing capabilities remain off.
- Add an immediate daemon-side kill switch
  `WG_PI_COMPACTION_KICK=0` (default unset/on for eligible workers). Check it at
  authorize **and permit**. Also inject the resolved setting into the child so
  the plugin avoids unnecessary calls.
- `wg pi-watchdog ... status` / `wg show --json` should expose only action IDs,
  bounded states/reason codes, counts, and guards—never prompt/summary text.
- Disabling after `Authorized` cancels it without charge. Disabling after
  `DeliveryPermitted` cannot unsend; it forbids all later permits and leaves an
  unacknowledged action uncertain. Disabling an acknowledged/running kick
  publishes cancellation and aborts it, but MUST retain the terminal/effect gate
  until abort/settlement/reap; a kill switch cannot turn off the safety
  interlock underneath an active run.
- Binary rollback procedure: set the kill switch, stop spawning new Pi workers,
  let current exact owners settle/reap, then install the prior binary/plugin
  pair. Old code must not be asked to interpret a live schema-v3 permit. Because
  historical compactions are never backfilled and old plugins cannot send, a
  later re-upgrade cannot duplicate an old action.

## 13. Required validation plan

### A. Red trace first

Before implementation, run the existing credential-free reproducer and retain
its failing trace:

```text
agent_end(willRetry=false)
compaction_start(reason=threshold)
compaction_end(reason=threshold, aborted=false, willRetry=false, result=true)
agent_settled
process exit 0
```

The implementation-facing assertion must fail pre-fix with the exact existing
text:

```text
RED: threshold compaction with explicit unfinished work must schedule one concrete post-compaction recovery turn (expected assistant marker FIXTURE_RECOVERY_TURN_EXECUTED after successful compaction_end(willRetry=false))
```

The standalone reproducer remains a non-WG control after the fix and therefore
must not start receiving WG kicks merely because the plugin is globally
installed.

A bounded **design-time host-contract probe** (not WG authority implementation)
was also run against the exact installed Pi 0.83.0 with the credential-free fake
provider plus a temporary final extension. It observed:

```text
WG_PROBE_QUEUE_READ pending=false
WG_PROBE_SEND_RETURN
WG_PROBE_MICROTASK
WG_PROBE_ACK action=wg-probe-action-1
```

The JSON stream contained one successful threshold `compaction_end`, then one
`agent_start`, one matching custom `message_start` with its action in `details`,
one recovery assistant marker, and one final `agent_settled`. The asserted order
`queue-read < send-return < queued-microtask < matching-message_start-ack` proves
both host assumptions are feasible on the pinned binary. Immutable validation
evidence carries the probe source, command/output, installed source digests, and
numbered dequeue/forwarding excerpts. The implementation MUST turn this bounded
probe into a permanent installed-flow regression; a different host still fails
closed rather than inheriting the observation.

### B. Decisive real flow

The new smoke must use:

1. the **real cargo-installed `wg`** to create/spawn a Pi task and generate the
   real wrapper;
2. the **real installed Pi** in `--mode json`;
3. the exact **embedded, cache-materialized plugin** passed by the production
   task-worker argv, with discovery disabled;
4. the credential-free fake provider/compaction hook loaded as an earlier
   explicit fixture extension;
5. real raw JSON and Pi v3 session files.

For the single-occurrence target, assert in order and before the first
`agent_settled`:

```text
compaction_end threshold success willRetry=false
agent_start
turn_start
matching wg-pi-compaction-kick custom message/action id
assistant FIXTURE_RECOVERY_TURN_EXECUTED
turn_end
agent_end
agent_settled
```

Also assert one OS Pi PID/one wrapper launch; unchanged task generation,
attempt ID/fence/sequence, worktree path/lease, session ID/file/branch lineage,
process epoch/PID digest, and frozen route; continuation epoch advances exactly
once; exactly one action/custom message/provider recovery turn exists; no retry
or breaker domain increments; usage is still accounted. Instrument the pinned
host to prove the final queue read and send occur in one call stack: a queued
message arriving while permit is awaited is observed/suppressed, while a
scheduled microtask cannot run between the final read and append. The same test
must fail closed when the host probe is forced unsupported.

This is a terminal/user-visible worker behavior fix. The smoke is the scripted
actual terminal flow, not only a Rust or TypeScript unit substitute, and its
manifest owner is grow-only.

The same real-flow smoke MUST include a second hardening phase whose first
recovery turn itself reaches another successful threshold compaction before
settlement. Assert the ordered subsequence
`E1 -> A1/message1 -> recovery-run-1 -> E2 -> A2/message2 -> recovery-run-2 ->
agent_settled`, with distinct Pi compaction entry IDs, `O1 != O2`, `A1 != A2`,
two custom messages, two provider recovery markers, one unchanged OS PID and
source/session/route tuple, and `continuation_epoch = c + 2`. Replay each entry's
plugin/raw events and both acks and assert the totals stay exactly two. Configure
the existing overall epoch/time budget high enough for two; do not add or relax
a special kick cap for this test.

### C. Negative controls

The installed-flow fixture plus integration/plugin tests must cover:

- manual compaction;
- overflow `willRetry=true` (one Pi-owned retry, zero WG kick);
- failed and aborted compaction;
- steering and follow-up already queued, queued while permit is awaited, and
  attempted callback/microtask interleaving at the final same-stack boundary;
- non-quiescent provider/tool activity and post-settled JSON;
- accepted success/failure/wait receipts before authorize and before permit;
- terminal after permit before acknowledgement (abort response), after
  acknowledgement with zero effects (revocation + cancel + abort-ack), and while
  an effect lease is open (not accepted until exact close);
- effect-begin versus terminal CAS in both orders, parallel terminal+sibling
  tool preflight, lost cancellation notification/reconnect, and abort-ack replay;
- unsafe/open/ambiguous effect lease and process exit while leased;
- session ID/file/leaf/fork mismatch;
- route/model/reasoning/plugin mismatch;
- process epoch/PID birth mismatch and process exit at each state;
- shared overall continuation epoch exhaustion and elapsed-time exhaustion,
  each loud as `HeldOperatorRequired`; no dedicated kick-count limit;
- duplicate/reordered `compaction_start`, `session_compact`, `compaction_end`,
  `agent_settled`, custom-message, and ack events;
- daemon crash before/after authorization, graph permit CAS, watchdog permit
  persistence/reply, send, and ack;
- plugin reload/session scan (ack only, never resend/backfill);
- normal managed final-answer control trace with no compaction (zero action even
  without classifying its prose), plus a threshold-compaction trace whose final
  answer has an accepted WG terminal receipt before permit (zero kick);
- the same final-sounding bytes followed by a qualifying threshold compaction
  but no WG receipt (one kick), proving prose cannot silently weaken receipt
  authority;
- a non-WG human Pi session in the same checkout/global-plugin environment.

### D. Repository gates

The implementation task must run at minimum:

```text
npm --prefix worksgood-pi test
make embed-worksgood-pi
git diff --exit-code worksgood-pi/embedded   # after a fresh re-embed
cargo fmt
cargo fmt --check
cargo clippy
cargo build
cargo test
tests/smoke/scenarios/pi_threshold_compaction_same_process_kick.sh
git diff --check
cargo install --path . --locked
```

The new smoke must fail against the pre-fix binary/embedded plugin with the red
trace and pass against the implementation. Existing overflow/manual/failure/
queued-follow-up controls must stay green.

## 14. Acceptance summary

This design addresses #6424 without teaching WG to read intent from model
prose. An ordinary final-answer trace with no threshold compaction creates no
action; a protocol-complete final has a terminal receipt and suppresses permit.
The only qualifying situation is a live, successful, non-retrying threshold
`session_compact` in an exact capability-bound task attempt whose WG lifecycle
is still unresolved. The lifecycle kernel grants one finite permit
per distinct qualifying compaction occurrence; the embedded plugin uses Pi's
existing in-process queue before JSON settlement inside a proven same-stack
queue-read/append critical section; a matching custom message event acknowledges
delivery. A lifecycle effect lease interlocks every recovery tool with terminal
CAS, so a later accepted terminal revokes the action, blocks new effects, and
aborts the run. Successive distinct occurrences get successive permits until
the existing overall epoch/time budget is loudly exhausted. All uncertain crash
windows stop rather than duplicate. The wrapper, generation, attempt, worktree,
session, process, and route remain the same.
