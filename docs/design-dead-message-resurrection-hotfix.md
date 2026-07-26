# Dead-message resurrection hotfix: pre-implementation quality gate

**Gate task:** `quality-pass-dead`

**Implementation task:** `stop-messages-from` (the graph contains no task ID named
`fix-dead-message-resurrection`)

**Status:** design approved only if the implementation and live regression obey
this document.

## Scope and rationale

The defect is a scheduler-authority error, not a message-retention error. A
message is durable user data. Its presence, unread state, age, or count must not
be evidence that a task can execute. Lifecycle state and an explicit persisted
wait subscription are the only scheduler authority.

The smallest safe hotfix therefore:

1. removes pending/unread-message predicates and “reopen/resurrection” actions
   from coordinator readiness, reconciliation, liveness, reaping, and admission;
2. preserves message append, read, delivery, and audit behavior;
3. permits one narrowly authorized resume request through a persisted
   `Waiting(Message)` subscription bound to the current attempt epoch; and
4. adds regression evidence at the real daemon ingress, without undertaking a
   general lifecycle, transport, chat, retry, or ownership redesign.

It must not delete, mark-read, rewrite, or hide messages to make readiness go
away. It must not auto-import an old attempt's inbox into a later attempt.

## Safety property

For every accepted message `M`, delivery may request a resume **if and only if**
all of these facts are true in the same atomic decision:

- the recipient task has a current, non-terminal attempt `A`;
- `A` is live according to lifecycle authority (message arrival itself is not a
  heartbeat and cannot make `A` live);
- `A.state` is explicitly persisted as `Waiting(Message, S)`;
- `M.recipient_attempt_epoch == A.epoch == S.attempt_epoch`;
- `M` matches subscription `S`'s persisted selector; and
- a compare-and-set changes `S` from armed to consumed for `M.id`.

The successful compare-and-set creates at most one idempotent resume request.
It does not let message storage directly assign ownership, create an attempt,
or spawn a process. A failed condition leaves the message retained and inert.
Termination, retry, and delivery races must fail closed under the same check.

A useful review formulation is:

```text
message_exists | unread_count | pending_count  != scheduler authority
current_epoch + live Waiting(Message) + armed matching subscription = resume authority
```

## Required identity and persistence

Every delivery decision needs stable identities for:

- message ID (for replay/duplicate idempotency),
- task/recipient ID,
- recipient attempt epoch captured when the message is accepted,
- subscription ID and subscription attempt epoch when a wait is armed, and
- consumed/resume-request ID when a subscription fires.

A task-targeted message is bound to the attempt observed at acceptance. If no
current attempt exists, its epoch is unbound and it is inert. Acceptance and a
concurrent retry must never cause a message to float to the new epoch. Displaying
the current epoch later is diagnostic only; it must not rewrite the binding.

## Behavior matrix

| Case | Message result | Task/attempt result | Resume/spawn result |
|---|---|---|---|
| Ordinary message to a live running agent | Retained and delivered normally | No status, epoch, liveness, readiness, ownership, or heartbeat mutation | None |
| Message to a dead agent/attempt | Retained as undeliverable/history for that epoch | Deadness and all lifecycle state unchanged | None |
| Message to a terminal task (`Done`, `Failed`, `Abandoned`, or equivalent) | Retained and inspectable | Terminal state remains terminal; no reopen/retry | None |
| Message bound to stale epoch `E` while current epoch is `E+1` | Retained against `E`; never retargeted | Both attempts unchanged | None |
| Exact replay of a message ID | Existing audit identity is reused/deduplicated | No repeated state transition | At most the original single resume request |
| Multiple distinct matching messages racing for one armed wait | All retained/audited | First atomic consumer may end the wait; later messages are inert for that consumed subscription | Exactly one resume request total |
| Nonmatching message to current live `Waiting(Message)` | Retained/delivered as applicable | Remains waiting | None |
| Matching message to current live `Waiting(Message)` at matching epoch | Retained and associated with consumed subscription | One authorized wait-to-resume-request transition | Exactly one resume; daemon restart/replay cannot repeat it |
| Historical unread/pending message seen after daemon restart | Still retained | No reconciliation, readiness, liveness, or ownership change | None |

“Live-agent delivery” and “wait-on-message” are intentionally different. A
running worker can receive messages without any scheduler transition. A waiting
attempt can request a resume only through the explicit subscription protocol.

## Coordinator boundary

Coordinator eligibility must be computed without inbox counts, unread flags,
pending flags, or message timestamps. Remove every path that treats those values
as generic readiness, including startup reconciliation and post-completion
reconciliation. No compensating message deletion is acceptable.

The explicit subscription handler may emit a typed resume request after the
atomic authorization above. The normal lifecycle/admission path remains
responsible for honoring that request. This separation prevents message arrival
from becoming an alternate hidden `reopen`, `retry`, ownership-transfer, or
spawn command.

Incoming messages also must not update agent heartbeat, task
`last_interaction`/liveness fields used by reaping, worktree ownership, current
attempt, or ready-queue position. Message-ledger timestamps may change.

## Operator diagnostic

The lowest-change existing message inspection surface should expose an exact,
machine-readable disposition with at least:

```json
{
  "message_id": "…",
  "recipient_id": "…",
  "recipient_attempt_epoch": 7,
  "current_attempt_epoch": 8,
  "disposition": "stale_epoch",
  "resume_requested": false,
  "subscription_id": null,
  "reason": "recipient attempt epoch 7 is not current epoch 8"
}
```

Allowed dispositions should distinguish at least `delivered_live`,
`dead_attempt`, `terminal_task`, `stale_epoch`, `duplicate`,
`waiting_nonmatch`, and `waiting_consumed`. Legacy/unbound data needs an
explicit `legacy_unbound` (or equally exact) disposition. The diagnostic reports
message state; it must not say or imply that “pending” means ready/runnable.
Adding a parallel lifecycle state machine solely for diagnostics is out of
scope.

## Compatibility and migration

- Preserve ordinary send/read APIs and message bodies. New metadata must be
  additive and tolerate old records.
- Historical messages are preserved in place. Missing recipient-epoch or
  subscription metadata is fail-closed as `legacy_unbound`, never inferred from
  whatever attempt happens to be current after upgrade.
- A legacy `Waiting(Message)` record without an explicit epoch and stable
  subscription identity is not resume-authorized. It must be explicitly
  re-armed by a live/current attempt or handled by an operator retry.
- No eager/destructive message migration, mark-read sweep, inbox purge, or
  terminal-task reopen is permitted. Derived diagnostic disposition may be
  computed lazily.
- A mixed-version coordinator fleet is unsafe because an old coordinator can
  still interpret pending data as readiness. Upgrade requires quiescing all
  coordinators, installing the hotfix, then restarting them. Writers/readers may
  remain compatible, but vulnerable schedulers may not remain active.
- No compatibility mode or feature flag may restore pending-message
  resurrection.

## Rollback

A plain operational rollback to a vulnerable daemon is unsafe while any pending
or historical messages exist. Safe rollback is:

1. stop/quiesce all coordinators;
2. retain the graph and message ledger unchanged for audit;
3. run read-only/manual tooling only; and
4. deploy a backported/patched prior binary (or forward-fix) before scheduler
   service resumes.

If an upgrade snapshot is used for disaster recovery, post-snapshot message
records must first be exported and preserved; restoring the snapshot must not be
presented as a valid way to erase the triggering data. Rollback documentation
must state explicitly that restarting the known-vulnerable daemon against the
unchanged graph is prohibited.

## Mandatory real-daemon regression

The permanent smoke must exercise the public CLI message ingress while a real
installed daemon is running. Calling a library method or editing an inbox file
is not equivalent to the human/operator flow.

### Harness

Use a deterministic fake worker executable that records every invocation and
PID to an append-only spawn ledger. Create fixtures for:

- a running live agent/current epoch;
- a dead attempt with no explicit message wait;
- terminal `Done`, `Failed`, and `Abandoned` tasks;
- a retried task with stale epoch `E` and current epoch `E+1`; and
- a current live attempt explicitly armed in `Waiting(Message, S)`.

Before delivery, capture canonical fingerprints of status, ready-list
membership, lifecycle/liveness fields, attempt count and epoch, ownership and
worktree mapping, daemon process/worker PIDs, and spawn-ledger count. Keep the
message ledger outside the no-change fingerprint because retention is the
expected write.

### Assertions

1. **RED-first:** on the vulnerable revision, reproduce `Done -> pending message
   -> Open/ready -> spawn` through CLI plus daemon and save the failing evidence.
2. Send many ordinary and duplicate/replayed messages to dead, terminal, and
   stale-epoch recipients. After multiple coordinator cycles, assert the
   fingerprints and spawn ledger are unchanged byte-for-byte, while every
   message remains inspectable with the correct disposition.
3. Send an ordinary message to the running live agent. Assert delivery, but no
   task/attempt transition, new PID, liveness refresh, ownership change, or
   spawn.
4. Send a nonmatching message to the armed wait; assert it remains waiting.
   Then deliver a matching message and concurrently replay it/send additional
   matching messages. Assert one consumed subscription, one resume request, and
   exactly one resulting resume/spawn.
5. Restart the daemon with historical unread/pending messages present. Assert
   zero reopen, readiness, ownership, attempt, liveness, or spawn change.
6. Assert the daemon event/transition trace contains zero intermediate
   reopen/retry/admission/ownership/spawn events for inert cases. This prevents
   a “click-through equivalent” bug in which final state happens to look the
   same after an unauthorized intermediate transition.
7. Exercise the exact operator-facing command flow (`wg msg …` plus daemon), not
   a direct helper. The scenario must be permanent in the grow-only smoke
   manifest and owned by `stop-messages-from`.

The explicit wait case is the only expected scheduler transition in the entire
matrix and must be asserted separately so it cannot mask inert-case changes.

## Review checklist for the implementation diff

Reject the hotfix if it:

- deletes or auto-reads messages to suppress readiness;
- retains any generic `pending_messages > 0 => ready/reopen/live` path;
- derives the recipient epoch at consume time instead of binding it at accept;
- treats message receipt as a heartbeat or updates ownership/current attempt;
- lets a later attempt inherit an earlier attempt's unread messages;
- performs a non-atomic “check wait, then wake” susceptible to duplicates;
- relies only on unit/CLI-without-daemon tests;
- checks only final status and not intermediate transition/spawn evidence;
- omits restart coverage, live ordinary delivery, or the positive explicit-wait
  control; or
- claims safe rollback by running the vulnerable coordinator unchanged.

Approval requires focused tests, format/lint/build, installed global binary,
and the owned real-daemon smoke in addition to this semantic review.
