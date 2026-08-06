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
   classified. “I am done” is not a WG receipt. A normal final answer is a
   must-not-trigger case when it has its accepted terminal receipt (or belongs
   to a non-WG session); a managed worker that emits final prose but omits its
   required WG receipt is intentionally still unresolved.
2. **Exactly one kick opportunity, at most one send invocation per action.** In
   the no-crash qualifying path the one durable action permits exactly one kick.
   Duplicate events, daemon replies, plugin reloads, and crashes do not create a
   second call to Pi for the same action ID. Distributed atomic exactly-once
   between two processes is not claimed: a crash after permit but before
   acknowledgement is held as indeterminate, never “fixed” by blind redelivery.
3. **Finite authority.** A kick consumes one existing Pi continuation epoch and
   its elapsed-time charge. The default dedicated cap is one compaction kick per
   source attempt, additionally bounded by the existing
   `max_continuation_epochs = 3` and `max_continuation_elapsed_secs = 1800`.
   The dedicated cap may later be configurable only within the existing hard
   bounds; the first implementation should keep it at one.
4. **Same owner.** A kick changes only `pi_continuation_epoch`. It does not
   change task generation, attempt ID/fence, attempt sequence, worktree path or
   lease epoch, Pi session ID/file/branch, process epoch, PID birth identity, or
   route snapshot.
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

### 3.1 Candidate ID

The daemon, not JavaScript, locates the named entry in the attested session file
and hashes its exact canonical JSON. It computes:

```text
candidate_id = b3(canonical-json([
  "wg.pi.threshold-compaction-candidate/v1",
  graph_id,
  task_id, generation, attempt_id, attempt_fence,
  worktree_lease_epoch,
  process_epoch, process_identity_digest,
  session_id, session_header_digest,
  compaction_entry_id, compaction_parent_id, compaction_entry_digest,
  route_snapshot_digest
]))
```

The candidate tuple is unique in the watchdog ledger. A repeated authorization
request first looks it up and returns that record; it never derives a fresh
record from the now-advanced continuation epoch.

### 3.2 Exactly-once durable action key

On the first authorization only, the watchdog captures the then-current
continuation epoch and frozen stock prompt:

```text
action_id = b3(canonical-json([
  "wg.pi.threshold-compaction-kick/v1",
  candidate_id,
  authorized_from_continuation_epoch,
  "WG_PI_COMPACTION_KICK_V1", prompt_digest
]))
```

The record fixes `to_continuation_epoch = from + 1`. The unique candidate index
and action ID together make these illegal:

- two action IDs for one compaction entry;
- one action applied to a different source/process/session/route;
- two epoch charges for a replayed permit;
- changed prompt bytes under an existing action ID.

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
| Budget | Dedicated kick count is below one and existing epoch + elapsed limits admit `from + 1` | `continuation_budget_exhausted`; operator required |
| Feature/host contract | Kill switch enabled; exact compatible embedded plugin loaded hermetically; supported host exposes awaited `session_compact`, custom message lifecycle, and active-window `followUp` behavior | No action; loud diagnostic |

The broker should reconcile bounded raw-stream observations to the current
complete line before evaluating effect/process evidence. Raw events never make
the unresolved-work decision and never create an action by themselves.

### Queue-race boundary

Pi exposes no atomic “queue is empty and append this message” API. WG closes the
practical race by making task workers hermetic: disable ambient extension
and settings discovery, load the embedded plugin explicitly and last, and give
one-shot JSON stdin EOF after the initial prompt. Earlier explicit extension
handlers (including the credential-free test provider) finish before the last
embedded `session_compact` handler, so their queued work is visible to the final
checks. If the launch permits an unbounded/background extension that can enqueue
later, the kick feature MUST be disabled for that launch. A pending message that
appears after authorization cancels that action; a message racing after the
permit consumes the permit but does not authorize a second send.

## 5. Protocol

Add three capability-scoped operations and one optional cancellation/status
operation. Suggested internal CLI spelling is:

```text
wg pi-watchdog compaction-kick authorize ...
wg pi-watchdog compaction-kick permit --action <id> ...
wg pi-watchdog compaction-kick ack --action <id> ...
wg pi-watchdog compaction-kick cancel --action <id> --reason <code>
```

They are not operator continuation commands. In worker mode they translate to
typed `WorkerOperation`s and cannot fall back to graph-file access.

### 5.1 Observe and authorize

The plugin's awaited `session_compact` handler performs its local guards and
calls `authorize` with only bounded identity fields: event reason/retry bit,
session ID, compaction entry ID/parent ID, current Pi PID, model identity,
plugin compat, and a deterministic request ID based on the entry ID. It sends no
summary or prompt text.

The broker authenticates the capability and exact process ancestry, reconciles
the session entry from disk, evaluates §4, and persists an `Authorized` watchdog
outbox record before replying. Authorization does not increment an epoch and is
safe to replay. A duplicate with identical fields returns the same action; a
duplicate candidate with different fields is a conflict.

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
`DeliveryPermitted` and only then returns the frozen prompt and permit receipt.
If the graph commit succeeded but the watchdog write/reply crashed, replay finds
the lifecycle audit event with this action ID and exact epoch, repairs the
outbox to `DeliveryPermitted`, and returns the same permit without another
charge. If the watchdog write happened but the graph CAS did not, no permit is
returned and replay retries the one CAS.

A terminal/park that commits first rejects the permit. A permit that commits
first authorizes at most this one delivery; a later terminal cannot rewrite
history, but it consumes all later continuation authority. The acknowledgement
response tells the plugin to abort the just-started run if a terminal/park won
after permit.

### 5.3 Enqueue and acknowledge

Immediately after receiving a permit, with no timer or event-loop deferral, the
plugin checks the same local identity/queue/quiescence tuple once more and calls
`pi.sendMessage(..., { deliverAs: "followUp", triggerTurn: true })` exactly
once. The action remains `DeliveryPermitted` during the unavoidable
send/ack gap; this state means “send may have happened,” never “safe to retry.”

The plugin registers `message_start`. When Pi selects a custom message whose
`customType`, action ID, prompt version, prompt digest, and content digest match
the permit, the handler calls `ack` before returning. This is the delivery
acknowledgement: the exact live Pi process dequeued the exact permitted message
for agent context. `ack` rechecks the source/process/session/action, persists
`Acknowledged`, and is idempotent. A later raw `agent_start`/turn and terminal
receipt are ordinary watchdog/lifecycle evidence, not additional authority.

If ack fails transiently after message selection, the plugin may retry **ack
only** on later matching message/turn events. It may never call `sendMessage`
again. On session reload it may acknowledge an already persisted matching Pi
custom-message entry, but it may not reconstruct or send an action from a
historical compaction entry.

The next public `agent_settled` after an acknowledged kick closes the action as
`SettledAfterKick`. If WG remains unresolved, the watchdog holds for existing
process-exit convergence/operator handling; it MUST NOT manufacture another
kick from settlement alone. A later distinct threshold compaction can qualify
only if the dedicated and shared budgets still allow it (the initial cap of one
means it will not).

## 6. Durable state machine

`PiCompactionKickRecord` is a bounded map/list in `PiWatchdogState`, keyed by
candidate and action IDs. Terminal states are retained for the source attempt.

| Current state | Input / condition | Transaction and next state | External action |
| --- | --- | --- | --- |
| `Absent` | `compaction_start` raw JSON | Record bounded diagnostic reason/start sequence only | None |
| `Absent` | `session_compact(threshold, willRetry=false)` and all authorize guards pass | Persist immutable record as `Authorized` | Return action metadata, no prompt authority yet |
| `Absent` | manual, overflow, pending queue, non-quiescent host, terminal WG state, unsafe effect, mismatch, disabled, or budget exhausted | No action, or bounded `Suppressed(reason)` diagnostic | None |
| `Absent` | failed/aborted `compaction_end` | Record diagnostic outcome; no candidate exists | None |
| `Authorized` | identical duplicate authorize | No mutation | Return same action |
| `Authorized` | changed payload for same candidate | `HeldConflict` | None |
| `Authorized` | local guard fails / queue appears | `Cancelled(reason)` | None; budget was not charged |
| `Authorized` | permit and terminal-clear CAS succeeds | Lifecycle epoch increments once; persist `DeliveryPermitted` | Return frozen prompt/permit |
| `Authorized` | terminal/park/process/mismatch/budget wins | `Cancelled` or `HeldMismatch` | None |
| `DeliveryPermitted` | duplicate permit | No increment | Return same permit |
| `DeliveryPermitted` | plugin invokes Pi API | State remains `DeliveryPermitted` (indeterminate until ack) | Exactly one local send call |
| `DeliveryPermitted` | exact custom `message_start`, terminal still clear | Persist `Acknowledged` | Pi continues existing process/session |
| `DeliveryPermitted` | exact custom `message_start`, terminal/park won after permit | Persist `AcknowledgedTerminalRace`; reply `abort=true` | Plugin calls `ctx.abort()`, never resends |
| `DeliveryPermitted` | process exits or session/process identity changes before ack | `Uncertain` | No redelivery; existing exit convergence |
| `Acknowledged` | duplicate ack/replayed message event | No mutation | None |
| `Acknowledged` | later `agent_start`/turn evidence | Optional `Running` diagnostic; same action | None |
| `Acknowledged` / `Running` | accepted WG terminal receipt | `TerminalObserved` | Existing finalization; no later permit |
| `Acknowledged` / `Running` | later `agent_settled`, no terminal | `SettledAfterKick`; hold `kick_completed_without_terminal` | No second settlement-derived prompt/kick |
| Any nonterminal action | exact process exit | `Authorized -> CancelledProcessExit`; `DeliveryPermitted -> Uncertain`; acked states retained | Wrapper/lifecycle exit path only |
| Any | duplicate raw lines / finished-stream replay | Cursor/action IDs make transition idempotent | Raw observer never sends |

`compaction_end` is still projected. A matching successful threshold end
confirms diagnostics. A missing end because the process died leaves the action
cancelled/uncertain according to its state. A contradictory end after a
`session_compact` (failure, abort, changed reason/retry) is a host-contract
violation: mark `HeldMismatch`, prevent all later permits, and surface it
loudly. It cannot authorize a retry.

## 7. Suppression table

| Must not over-trigger | Explicit rule |
| --- | --- |
| Normal final answer | Accepted `wg_done`/success terminal receipt wins. Prose is ignored. A final answer in an unmanaged Pi session has no capability and no handler registration. |
| Manual compact | `event.reason != "threshold"`; ignore even when successful and idle |
| Overflow | `willRetry == true` (and/or reason `overflow`); Pi already owns its one retry |
| Failed compaction | No successful `session_compact` entry; `compaction_end.result` absent/error |
| Aborted compaction | No successful qualifying entry; `aborted == true` diagnostic only |
| Queued steering/follow-up | `ctx.hasPendingMessages()` at any of the three local checks cancels/suppresses; raw queue counts corroborate only |
| Non-idle provider/tool run | Require the post-`agent_end`, pre-settlement `CompactionQuiescent` automaton and no open tool. A random `session_compact`/reload/timer cannot send |
| Already idle/settled JSON | `ctx.isIdle() == true` is rejected because it would start a detached prompt that print mode does not await |
| `wg_done` / `wg_fail` / `wg_wait` | Lifecycle first-terminal/park CAS before permit cancels. If it races after permit, no later permit is possible and ack requests abort |
| Unsafe in-flight effect | Open/unsafe/receipt-ambiguous `ToolContract` or false effect guard holds the action |
| Route mismatch | Frozen route digest/model/reasoning/plugin artifact must match; profiles/config are not re-resolved |
| Session/branch mismatch | Exact ID/header/file/current compaction leaf/prefix must match; no fork/resume selector substitution |
| Process mismatch/exit | Exact process epoch and PID birth identity must match; no replacement process is launched |
| Exhausted continuation budget | Dedicated per-attempt cap and shared epoch/elapsed limits both must pass |
| Duplicate/replayed event | Candidate/action unique keys return the existing terminal state |
| Non-WG human/chat Pi | Missing attempt capability or chat binding: continuation module is inert and performs no broker call |
| Historical compaction on upgrade/reload | Never backfill; only a live awaited qualifying callback may authorize |

## 8. Crash and race matrix

The table uses `A` = durable `Authorized`, `P` = lifecycle CAS committed and
`DeliveryPermitted`, `S` = Pi API send invoked, and `D` = durable delivery ack.

| Crash/race point | Durable evidence after recovery | Replay behavior | Outcome |
| --- | --- | --- | --- |
| Before `session_compact` / during `compaction_start` | No action; optional bounded start diagnostic | Do not infer from start | Process exit/convergence or Pi's normal behavior |
| Compaction fails/aborts before saved entry | No action | Replayed end remains diagnostic | No kick |
| Saved entry, crash before `A` | Session may contain compaction; no live authorization record | Do **not** backfill from session | No kick; process-exit convergence |
| After `A`, before permit request | `Authorized`, epoch unchanged | Same live process/callback may request permit once; a restarted/different process cancels stale `A` | Safe retry of authorization/permit, no send yet |
| Terminal/park before permit CAS | Terminal receipt; `A` may exist | Permit rejects and marks cancelled | Terminal wins, no send |
| During permit before graph commit | `A`, old epoch | Replay retries same CAS/action ID | At most one epoch charge |
| Graph permit commit, crash before watchdog `P`/reply | Lifecycle audit has action ID and new epoch; watchdog has `A` | Reconcile to `P`; same live caller may receive same permit. Different/dead process becomes uncertain/cancelled | Permit is not charged twice |
| After permit reply, before `S` | `P`; send unknown | Never automatically resend. If the original handler is still executing it may make its one immediate call; after restart/exit mark uncertain | At-most-once beats liveness |
| During/after `S`, before matching `message_start` | `P`; Pi queue may contain action | Do not send again. Original process may drain and ack; reload may only inspect/ack a persisted matching custom entry | No duplicate; uncertain on exit |
| Matching message selected, crash during ack RPC | Pi observed exact action; watchdog may still show `P` | Retry ack only from matching events/session entry; never send | Eventually ack or remain uncertain |
| After `D`, before provider/assistant response | `Acknowledged` | Duplicate authorize/permit/ack are no-ops | Never redeliver; process exit uses convergence |
| Terminal commits after `P`, before `D` | Permit and terminal both ordered; no further authority | Matching ack returns `abort=true`; plugin aborts if possible | One committed kick maximum, no loop |
| Terminal commits after `D` | Ack plus terminal | Normal first-terminal finalization; no further permit | Terminal wins future work |
| Public `agent_settled` before a kick | This means delivery did not enter the required active callback | Do not send post-settled; hold and let wrapper exit | Avoid JSON detached-run loss |
| Public `agent_settled` after `D` | `SettledAfterKick` | No settlement-derived second prompt | Finite stop/operator/convergence |
| Process exits with `A` only | `CancelledProcessExit` | No permit on a new PID | Existing new-attempt convergence |
| Process exits with `P` but no `D` | `Uncertain` | Never redeliver same action | Existing new-attempt convergence |
| Process exits after `D` unresolved | Delivered action plus exact reap | No same-action retry; convergence may create a **new generation/attempt** under existing rules | Ownership boundaries stay explicit |
| Duplicate raw/plugin events | Same stream cursor/candidate/action/request IDs | Return stored state or conflict on changed bytes | No extra send/charge |
| Daemon restarts | Watchdog outbox + lifecycle audit survive | Reconcile `A`/`P` ordering before replying | Same action only |
| Plugin reload, same process | Session scan may find a persisted custom action | Ack existing action only; never authorize from historical compaction | No replay send |

This is intentionally at-most-once at the Pi API boundary with an explicit
`Uncertain` state. Claiming guaranteed exactly-once delivery across the `P -> S
-> D` crash window would be false because Pi exposes neither a transactional
queue API nor a send promise/receipt to extensions.

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
| continuation epoch | `c` | `c + 1` exactly once at permit |
| retry/admission/breaker/eval/accounting domains | current values | no increments caused by the kick (ordinary Pi usage still accounts normally) |

The raw observer must project the second run's ordinary usage once. The kick
itself adds no source retry or replacement-process accounting.

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
  - add `PiCompactionKickRecord`, states, candidate/action derivation,
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
  - no new retry domain and preferably no new transition kind;
  - preserve first-terminal semantics and reuse
    `PiContinuationEpochReserved`;
  - if the implementation cannot recover the action ID from lifecycle audit,
    add an optional `action_id` field (serde-defaulted) to that transition and
    bump `LIFECYCLE_SCHEMA_VERSION` from 1 to 2. Do not create a parallel
    continuation authority.
- `src/worker_control.rs`, `src/worker_cli.rs`,
  `src/commands/service/ipc.rs`, and `src/cli.rs`
  - add typed authorize/permit/ack/cancel payloads;
  - add a dedicated `WorkerOperationKind::PiCompactionKick` to newly issued
    capabilities; do not silently broaden already issued capabilities;
  - use deterministic request IDs for all four operations.
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
  - local guard checks, authorize/permit/send-once/ack-only replay;
  - no registration without the exact worker capability/task-worker launch
    contract.
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
  - registration, quiescent-window qualification, queue/terminal/mismatch
    suppression, send once, ack-only replay, and host-contract assertions.

### Tests

- `tests/integration_pi_watchdog.rs`: state transitions, exact key, lifecycle
  CAS, terminal/wait races, effect/session/route/process guards, budget, and all
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

Prefer reusing lifecycle's existing continuation authorization and epoch
transition. If an optional action ID is added to persisted lifecycle events,
serde-default old events and bump the lifecycle schema as described in §11.
There is no task graph status migration and no new task generation/attempt.

The typed worker operation is additive. Keep
`WORKER_CONTROL_PROTOCOL = worksgood-worker-control-v2` if old peers already
fail closed on unknown operation tags; otherwise bump it once and migrate the
daemon/CLI together. In either case, old capabilities lacking the dedicated
operation cannot kick and must wait for a newly spawned attempt.

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
  unacknowledged action uncertain.
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

For the target, assert in order and before the first `agent_settled`:

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
or breaker domain increments; usage is still accounted.

This is a terminal/user-visible worker behavior fix. The smoke is the scripted
actual terminal flow, not only a Rust or TypeScript unit substitute, and its
manifest owner is grow-only.

### C. Negative controls

The installed-flow fixture plus integration/plugin tests must cover:

- manual compaction;
- overflow `willRetry=true` (one Pi-owned retry, zero WG kick);
- failed and aborted compaction;
- steering and follow-up already queued;
- non-quiescent provider/tool activity and post-settled JSON;
- accepted success/failure/wait receipts before authorize and before permit;
- terminal after permit before/after acknowledgement (abort response);
- unsafe/open effect;
- session ID/file/leaf/fork mismatch;
- route/model/reasoning/plugin mismatch;
- process epoch/PID birth mismatch and process exit at each state;
- dedicated and shared continuation budget exhaustion;
- duplicate/reordered `compaction_start`, `session_compact`, `compaction_end`,
  `agent_settled`, custom-message, and ack events;
- daemon crash before/after authorization, graph permit CAS, watchdog permit
  persistence/reply, send, and ack;
- plugin reload/session scan (ack only, never resend/backfill);
- normal final response with an accepted WG terminal receipt;
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
prose. The only qualifying situation is a live, successful, non-retrying
threshold `session_compact` in an exact capability-bound task attempt whose WG
lifecycle is still unresolved. The lifecycle kernel grants one finite permit;
the embedded plugin uses Pi's existing in-process queue before JSON settlement;
a matching custom message event acknowledges delivery. All uncertain crash
windows stop rather than duplicate. The wrapper, generation, attempt, worktree,
session, process, and route remain the same.
