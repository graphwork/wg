# Simplified authoritative task lifecycle

**Status:** Ratified design

**Date:** 2026-07-26

**Input:** [`docs/studies/task-lifecycle-coordinator-deep-survey.md`](studies/task-lifecycle-coordinator-deep-survey.md)

**Scope:** task/run lifecycle, attempts, coordinator reconciliation, worktrees, messages, and evaluation

**Out of scope:** production changes in this task

## 1. Decision

WG will have exactly **one task-transition authority**: a pure `LifecycleKernel::transition` function committing typed events to an append-only lifecycle ledger under `graph.lock`. Every CLI command, wrapper, daemon phase, evaluator, remote provider, cron controller, and recovery command becomes a requester. None may assign task status directly.

The current task row remains a materialized compatibility projection. The ledger is authoritative. A task consists of immutable identity/configuration plus numbered **generations**. A terminal generation is never reopened. Retry, reset, replay, cron, and cycle continuation create a new generation through an explicit, attributable event.

The ratified state domains are independent:

1. task-generation state;
2. execution-attempt state;
3. worker-process observation;
4. resource-admission decisions;
5. provider-health observations;
6. worktree lease and merge state;
7. evaluation jobs and append-only evidence; and
8. reconciliation issues and side-effect outbox state.

No domain is encoded as another. In particular:

* capacity, disk pressure, dependency blocking, provider health, and ownership conflicts do not become task failure;
* process launch/runtime failure terminalizes a failed **attempt** and its generation;
* evaluator launch/runtime failure changes only the evaluation job/evidence record;
* merge conflict changes only the merge/acceptance hold, not source-attempt success;
* a message is immutable data unless it satisfies a correlation-bound wait already persisted for the current generation; and
* `Done`, `Failed`, and `Abandoned` are terminal for one generation. Only an authorized `GenerationCreated` event can make later execution possible.

This reduces the **44 production files in the direct-writer inventory** in survey §3.3 to **one production module and one transition entry point**. Constructors and migrations also call that entry point. CI will reject direct task-status assignments elsewhere.

### 1.1 Non-negotiable outcomes

The following are not configuration choices:

* The reproduced `Done -> Open` pending-message path is deleted.
* Completion and failure are first-terminal-wins per attempt.
* Late or contradictory wrapper output is retained as evidence and cannot change state.
* Evaluation evidence is append-only and cannot reopen source execution.
* Worktree ownership is a fenced lease. Terminalization atomically records retention/release; reuse atomically transfers the lease to a new attempt.
* A stale owner, stale claim, reset race, or worktree ownership conflict is breaker-neutral. It creates one deduplicated reconciliation issue and an exact readiness hold, never repeated spawn failures.
* Resource denial is a deferral, not an attempt and not a failure.
* There is no model-based triage path to `Done`.

## 2. Goals and non-goals

### 2.1 Goals

1. Make every lifecycle mutation legal, attributable, idempotent, and replayable.
2. Preserve durable worker recovery, attempt history, source-bearing worktrees, messages, cycles/cron, remote execution, and optional evaluation.
3. Make crash recovery a deterministic replay/reconciliation protocol rather than a collection of inferred status rewrites.
4. Make task readiness explainable from the same ordered gates used by dispatch.
5. Land incrementally without rewriting existing graphs in place or invalidating existing command names.
6. Make every current failure class end in one deterministic domain/state and reason code.

### 2.2 Non-goals

* Moving all persistence to SQLite.
* Making Git, processes, remote providers, and the graph one distributed transaction.
* Deleting preserved worktrees automatically.
* Treating process liveness as proof of useful work or task success.
* Replacing the message transport or evaluation model plane.
* Retroactively proving facts that old graph rows did not record.

## 3. Terminology and invariants

A **task** is the durable unit and its configuration. A **generation** is one semantic execution run of that task. A **dispatch attempt** is one exclusive worker lease within a generation. Waiting may park one attempt and later dispatch another attempt in the same generation from a checkpoint. Retry/reset/replay/cycle/cron create a new generation.

A **fence** is a monotonically increasing token on the task. Every attempt and exclusive worktree lease carries `(task_id, generation, attempt_id, fence)`. A requester lacking the current tuple has no mutation authority.

An **acceptance record** is the typed proof that a successful source attempt met the generation's pinned completion policy. It references dependency revisions and content-addressed deliverable, test, merge, remote-attestation, manual-override, and required-evaluation evidence as applicable.

The kernel enforces:

```text
K1  one task has at most one nonterminal execution attempt
K2  Running => current_attempt exists and carries the task's current fence
K3  only the matching attempt may request success, failure, or park
K4  the first terminal disposition of an attempt wins
K5  a later contradictory report is evidence only
K6  a terminal generation never changes state
K7  later execution requires GenerationCreated and a greater generation number
K8  Done requires one acceptance record satisfying the pinned policy
K9  resource/provider/dependency holds never synthesize task or attempt failure
K10 one matching message consumes one current wait receipt at most once
K11 no message can create a generation or mutate a terminal generation
K12 source-bearing acceptance requires an integrated or explicitly deferred merge receipt
K13 worktree lease changes and the lifecycle event that motivates them commit together
K14 stale ownership/reconciliation issues do not increment launch-failure budgets
K15 every accepted event and all projection changes commit under graph.lock
K16 replaying an event or reconciliation observation twice is semantically a no-op
K17 required evaluation is bound to task, generation, source attempt, policy, and route
K18 evaluation evidence never changes the source attempt's terminal disposition
```

## 4. The single authority

### 4.1 API

All mutation flows use one conceptual API:

```rust
fn transition(
    snapshot: &LifecycleProjection,
    request: TransitionRequest,
) -> Result<CommitPlan, TransitionRejection>;
```

The I/O wrapper takes `graph.lock`, reloads the current projection, calls the pure function, durably appends the accepted event, applies the event to materialized projections, and saves the compatibility graph. The pure function:

1. authenticates the actor class;
2. checks the idempotency key;
3. compares expected task revision, generation, attempt, and fence;
4. validates the legal edge and pinned policies;
5. validates referenced evidence digests/receipts;
6. normalizes all transition-owned fields;
7. atomically changes related attempt and worktree-lease records;
8. emits idempotent outbox actions; and
9. returns a structured rejection without mutation if any check fails.

A rejection has a stable code such as `stale_attempt`, `attempt_already_terminal`, `generation_terminal`, `dependency_revision_changed`, `lease_not_quiescent`, or `acceptance_evidence_missing`. Rejected reports may append a deduplicated `EvidenceObserved` event, but never a lifecycle edge.

### 4.2 Ledger

Each accepted record contains at least:

```text
event_id, idempotency_key, schema_version
sequence, task_id, task_revision, generation
request_kind, event_kind, old_state, new_state
actor_kind, actor_id, attempt_id, fence
reason_code, policy_id
input_event_ids, evidence_refs, outbox_action_ids
occurred_at, committed_at
```

The target store is an append-only checksummed ledger at `.wg/lifecycle/events.jsonl`, serialized with the existing `graph.lock`. An event is fsynced before its graph projection is acknowledged. The graph snapshot is then atomically replaced as today. A crash after ledger append but before projection save is repaired by replay. The kernel never writes a projection before its event. A torn final ledger frame is unacknowledged and truncated to the last checksum-valid frame on recovery.

`.wg/graph.jsonl` remains the compatibility/read projection during migration. Its lifecycle revision and ledger head identify exactly which events it includes. Other immutable evidence (verdicts, merge receipts, output manifests) remains content-addressed outside the ledger and is referenced by digest.

### 4.3 Requesters, not writers

| Requester | Requests it may make | Requests it may not make |
|---|---|---|
| CLI/operator | publish/pause policy, wait, cancel, abandon, explicit retry/reset/replay, explicit override/waiver, conflict resolution | direct status assignment; unfenced completion of a live attempt |
| dispatcher | reserve an admitted attempt; publish a launch permit | success, failure, retry, or acceptance |
| current worker/wrapper | completion intent, explicit failure, park/wait intent, progress evidence | retry, generation creation, acceptance, or mutation after losing its fence |
| process observer | append spawn/exit/heartbeat observations; request `AttemptLost` with exact process proof | infer completion from output prose; triage to `Done` |
| wait matcher | request one `WaitSatisfied` with matching correlation and barrier | wake non-waiting or terminal work |
| acceptance controller | request `AcceptanceSatisfied` or `AcceptanceRejected` from a pinned deterministic policy and exact evidence | reopen source execution |
| evaluation runner | append evaluation job observations and verdict evidence | task status, source attempt, retry, or worktree mutation |
| retry controller | request a new generation only when a persisted retry policy authorizes the exact failed generation | unbudgeted rescue or retry by inference |
| cron/cycle controller | request a new generation from a persisted due schedule/iteration rule | rewrite a terminal generation |
| reconciler | request one proof-bound lost-attempt/fence/outbox reconciliation event; create/resolve issues | accept work, invent output, retry absent policy, charge breakers for conflicts |
| remote provider adapter | append signed process/result evidence for its current remote attempt | directly mark a local task `Done` |
| importer/migrator | append typed import/checkpoint events with provenance/confidence | silently normalize ambiguous history |

No actor receives a general `set_status` capability. Public library APIs expose requests and read models only.

## 5. Independent state domains

### 5.1 Task-generation state

```text
Open                 published generation has no exclusive attempt
Running              current exclusive attempt exists
Waiting              explicit persisted wait; prior attempt is parked
AwaitingAcceptance   source attempt succeeded; hard acceptance policy unresolved
Done                 accepted terminal result
Failed               terminal execution or semantic rejection
Abandoned            explicit terminal skip
```

`Done`, `Failed`, and `Abandoned` are terminal for that generation. `AwaitingAcceptance` is not an evaluation state: it means the task's pinned acceptance policy is unresolved and may be waiting on merge, manual approval, tests, or required evaluation.

Publication, pause, `not_before`, dependency disposition, and other eligibility facts are policy/readiness fields, not extra task states. Routine `Blocked`, `Incomplete`, `PendingValidation`, `PendingEval`, and `FailedPendingEval` are retired from the canonical enum.

Dependency satisfaction is explicit:

* `Done` satisfies.
* `Failed` does not satisfy.
* `Abandoned` satisfies only when its event's `satisfy_dependents=true`; the compatibility preset keeps today's default.
* Every other state does not satisfy.

### 5.2 Attempt state

```text
Reserved -> Preparing -> LaunchPermitted -> Running
Running  -> Succeeded | Failed | Parked | Cancelled | Lost
Preparing/LaunchPermitted -> Failed | Cancelled | Lost
```

The terminal attempt dispositions are immutable. `Succeeded` means source execution produced a completion proposal; it does not mean the task was accepted. `Failed` carries a typed class and evidence. `Parked` carries the checkpoint and wait ID. `Lost` is used only after exact process identity can no longer be live. `Cancelled` is an explicit fence operation.

An attempt record owns route/model/reasoning, process identity, output manifest, session reference, worktree lease, timestamps, cost, and failure evidence. `retry_count`, `dispatch_count`, rapid-death count, and circuit-breaker totals become derived views over attempts rather than competing notions of identity.

### 5.3 Worker-process state

Process records are observations:

```text
Reserved, SpawnGated, Spawned, Alive, Exited(code), Signaled, Unknown
```

They include PID plus process-start identity/nonce, not PID alone. Process state cannot mark a task successful. A launch syscall failure or a nonzero/lost runtime is evidence for a canonical attempt-failure request. A zero exit without completion evidence is `AttemptFailed(NoCompletionProtocol)`, not success.

### 5.4 Resource admission

Admission produces expiring records:

```text
Granted(snapshot_id, expires_at)
Deferred(reason, snapshot_id, retry_after)
```

Reasons include global slots, priority queue, disk projection, heavy-builder budget, route/provider pause, and configured time gates. A deferral creates no attempt, changes no task state, consumes no attempt/retry/spawn budget, and is re-evaluated from a fresh snapshot.

Invalid task configuration is a `DispatchIssue`, not resource pressure. The task remains `Open` but ineligible until configuration changes. `wg why-not-ready` distinguishes these cases.

### 5.5 Provider health

Provider health is an independent, expiring observation keyed by route/provider:

```text
Healthy, Degraded, Unavailable, Unknown
```

It may cause admission deferral. It never fails an already-running attempt without process/result evidence, and it never becomes a task status. Breaker counters count provider observations or real failed attempts, never ownership conflicts.

### 5.6 Worktree lease and merge state

A worktree record has immutable identity/path/branch plus a monotonically increasing `lease_epoch`:

```text
Available
Active(attempt, fence, lease_epoch)
Sealing(attempt, fence, lease_epoch)
Sealed(from_attempt, lease_epoch)
MergePending(receipt_request)
MergeConflict(conflict_digest)
Integrated(merge_receipt)
Retained(from_attempt, reason)
Quarantined(issue_id)
CleanupPending(merge_receipt)
```

Only the kernel changes logical lease state. Only the worktree outbox consumer may perform physical create, merge, archive, or cleanup, and every operation supplies the expected lease epoch.

Rules:

1. Attempt reservation and acquisition/transfer to `Active` are one ledger transaction.
2. A completion, failure, or park intent from a live handler is only an intent. The wrapper finalizes it after the handler is quiescent. Success terminalizes the attempt and changes `Active -> Sealed` atomically.
3. A failed/lost/cancelled attempt changes `Active -> Retained` atomically only after quiescence is proved. If logical authority has been revoked but the process may still write, the lease becomes `Quarantined(issue_id)` and cannot be reused. Retention is deliberate and source-preserving, not an owner leak.
4. Retry-in-place transfers `Retained(old) -> Active(new)` in the same transaction that reserves the new attempt, and only after the old process fence is proved.
5. A fresh retry leaves the old lease `Retained` and allocates a different worktree.
6. `Done` atomically records an accepted merge receipt and changes the lease to `Integrated/CleanupPending`. Physical deletion remains best effort and never affects task status.
7. A conflict leaves the successful attempt unchanged and the task `AwaitingAcceptance`; the lease is `MergeConflict` until an explicit resolution/deferred-merge receipt.
8. Unknown physical ownership creates `Quarantined(issue_id)`. It is a breaker-neutral readiness hold. Dispatch does not retry it every tick.

Compatibility `wg done` invoked from a live source worker becomes a completion **intent**. The wrapper finalizes it after handler quiescence. Operator completion first cancels/fences any live attempt. This closes the stale process/worktree race instead of relying on cooperative timing.

### 5.7 Evaluation jobs and evidence

Evaluation is not represented by task status or ordinary graph task state. See §10.

### 5.8 Reconciliation issues and outbox

A `ReconciliationIssue` is a deduplicated record, not a task state:

```text
issue_id, task, generation, attempt, kind, evidence
first_seen, last_seen, state(Open|Resolved|OperatorRequired)
readiness_effect, suggested_operator_command
```

Examples are ambiguous process identity, stale claim, projection mismatch, worktree owner conflict, missing merge receipt, and reset awaiting a fence. Reobserving the same signature updates `last_seen`; it does not create another launch failure.

Outbox actions carry stable IDs and expected fences. Side-effect consumers write receipts. Reprocessing an action or receipt is idempotent.

## 6. Canonical transitions

| Event | Required prior task state | Task result | Attempt result | Authorized requester |
|---|---|---|---|---|
| `TaskCreated` | absent | `Open` (possibly unpublished) | none | add/import through kernel |
| `AttemptReserved` | `Open`, all readiness gates clear | `Running` | new `Reserved` | dispatcher |
| `LaunchPermitted` | `Running`, matching reserved attempt/lease | unchanged | `LaunchPermitted` | dispatcher |
| `AttemptRunning` | matching permit/process | unchanged | `Running` | process observer |
| `AttemptSucceeded` | `Running`, matching current fence | `AwaitingAcceptance` | `Succeeded` | quiescent wrapper/remote adapter |
| `AttemptFailed` | `Running`, matching current fence | `Failed` | `Failed(class)` | worker or proof-bound observer |
| `AttemptLost` | `Running`, exact dead identity, no terminal event | `Failed` | `Lost` | reconciler |
| `AttemptParked` | `Running`, matching current fence | `Waiting` | `Parked` | worker/operator |
| `WaitSatisfied` | `Waiting`, exact generation/wait/correlation | `Open` | unchanged terminal parked attempt | wait matcher/operator condition |
| `AcceptanceSatisfied` | `AwaitingAcceptance`, policy complete | `Done` | unchanged `Succeeded` | acceptance controller |
| `AcceptanceRejected` | `AwaitingAcceptance`, valid hard evidence | `Failed` | unchanged `Succeeded` | acceptance controller |
| `AbandonRequested` | any nonterminal state | `Abandoned` after fencing | active attempt `Cancelled` if any | operator |
| `GenerationCreated` | terminal generation | new generation `Open` | none | operator or exact pinned retry/cron/cycle policy |
| `CancelRequested` | `Running` | remains non-ready while fencing | cancel pending observation | operator/controller |
| `FenceEstablished` | cancel/reset pending | atomically terminalizes the old generation as `Abandoned(Superseded)` and may create the requested new `Open` generation | `Cancelled` | process observer/operator proof |
| `EvidenceObserved` | any | unchanged | unchanged | any authenticated adapter |

A reset of a running task is a protocol, not `Running -> Open`: `CancelRequested`, process/worktree fence, then one transaction that records `FenceEstablished`, terminalizes the old generation as explicitly superseded, and appends `GenerationCreated`. Reset of a non-running nonterminal generation likewise records it as superseded before creating the next generation. Until the fence is established, readiness reports `reconciliation.reset-awaiting-fence`. A force action may revoke the logical token immediately but cannot transfer the same physical worktree until process quiescence; it therefore remains held or allocates a fresh path.

### 6.1 First-terminal-wins

The terminal attempt event is a compare-and-set on `(task, generation, attempt, fence, terminal=None)`. The first accepted `Succeeded`, `Failed`, `Parked`, `Cancelled`, or `Lost` disposition wins. A duplicate request with the same idempotency key returns the original event. A different later request returns `attempt_already_terminal` and stores only `EvidenceObserved(late_*)`.

This applies to the nightmare sequence from the survey: a stall/lost event accepted first makes a late wrapper `done` stale evidence. If success was accepted first, a later process-exit failure is process evidence and cannot rewrite it.

A generation's first terminal task state also wins. Acceptance cannot change `Failed` to `Done`, and evaluation cannot change `Done` to `Failed` or either terminal state to `Open`. A later run requires a new generation.

### 6.2 Explicit generation creation

Only these causes may create a new generation:

* operator `retry`, `reset`, `replay`, or archive restore;
* a persisted automatic retry policy naming the failed generation and remaining budget;
* a persisted cron schedule firing a unique occurrence ID; or
* a persisted cycle policy advancing a unique iteration ID.

Each cause has an idempotency key. A retry controller emits `RetryAuthorized(policy_id, failed_generation)` before `GenerationCreated`; there is no inference from a task merely being failed. The default for new ordinary tasks is manual retry. Existing explicit `max_retries` migrates to a pinned automatic policy.

Messages, evaluator verdicts/failures, process heartbeats, provider recovery, config changes, and daemon restart are not generation authorities.

Cron and cycles therefore stop rewriting the same terminal run. Their prior generation remains terminal and auditable; the task's compatibility status shows the newest generation.

## 7. Readiness and admission

Readiness is a pure, ordered gate pipeline shared by dispatcher and diagnostics:

1. current generation is `Open`;
2. task is published and not paused;
3. temporal/cron occurrence is due;
4. local and remote dependencies have a satisfying disposition;
5. no current attempt, unresolved fence, or reconciliation issue dominates execution;
6. wait/acceptance state is not active;
7. task class is dispatcher-managed;
8. route/profile/configuration resolves;
9. provider breaker permits consideration;
10. global capacity and priority permit a slot;
11. resource projection permits the task;
12. workspace lease can be acquired; and
13. the final locked revision/fence still matches.

Gates 8 and 12 can produce persistent diagnostic issues. Gates 9–11 produce expiring deferrals. None changes task status. The dispatcher reserves an attempt only after admission, so denied admission cannot create a failed attempt.

A route resolved successfully but whose process cannot be launched creates a real reserved attempt and then `AttemptFailed(LaunchProcess)`. Registry/permit persistence failure after reservation is `AttemptFailed(LaunchInfrastructure)` unless exact rollback proves the permit was never publishable; either way it is one attempt, not multiple counter increments.

## 8. Deterministic failure classification

This table is normative and covers the failure/hold families inventoried by the survey.

| Current class | Canonical domain and outcome | Task-generation effect |
|---|---|---|
| unpublished/paused/not-before/cron not due | readiness policy hold | none (`Open`, not ready) |
| open/failed/missing local dependency | dependency gate with exact disposition | none |
| unresolved/failed remote dependency | dependency gate/`DispatchIssue` | none |
| cycle external blocker | dependency gate | none |
| stale assignment or graph/registry mismatch | one `ReconciliationIssue(StaleOwnership)` | none; breaker-neutral hold |
| daemon-managed task class | readiness policy owner | none |
| unresolved assignment/assignment-job failure | pinned assignment readiness policy/`DispatchIssue(Assignment)` | none; no hidden assignment-task transition |
| full worker slots/priority loss | `AdmissionDeferred(Capacity)` | none |
| disk projection/heavy builder budget | `AdmissionDeferred(Resource)` | none |
| provider breaker/zero-output provider pause | provider/admission deferral | none |
| invalid profile/model/endpoint/credential before reservation | `DispatchIssue(Configuration)` | none; operator/config change required |
| worktree owner/path/branch collision | `ReconciliationIssue(WorktreeConflict)` + quarantine | none; breaker-neutral hold |
| worktree creation I/O after reservation | `AttemptFailed(WorkspaceProvisioning)` | `Failed` |
| OS process launch/permit/registry launch failure | `AttemptFailed(LaunchProcess|LaunchInfrastructure)` | `Failed` |
| rapid death, signal, nonzero runtime | `AttemptFailed(RuntimeExit)` | `Failed` |
| zero output/no completion protocol | `AttemptFailed(NoUsefulOutput|NoCompletionProtocol)` | `Failed` |
| heartbeat/PID identity conclusively lost | `AttemptLost` | `Failed` |
| liveness ambiguous | `ReconciliationIssue(ProcessIdentityAmbiguous)` | none until proof/operator action |
| worker explicit failure | `AttemptFailed(SourceExecution)` | `Failed` |
| stale worker complete/fail/wait | rejected request + late evidence | none |
| deliverable/test/verify missing while worker live | completion request rejected with acceptance diagnostic | remains `Running`; it may correct and retry the intent |
| deliverable/test/verify missing after handler quiescence | `AttemptFailed(CompletionPreflightRejected)` | `Failed`; evidence names the rejected gate |
| dependency revision changes during completion | completion request rejected | remains `Running`, or fails preflight if already quiescent |
| uncommitted worktree | completion request rejected | remains `Running`; source preserved |
| merge conflict after source success | merge record `MergeConflict` | `AwaitingAcceptance` |
| push/archive/audit ancillary failure after accepted merge | outbox issue/repair; acceptance receipt remains authoritative | no rollback; loud issue |
| remote signature/scope/lease invalid | completion request rejected or remote attempt failed | `Running` or `Failed` by exact evidence |
| required evaluator low/reject verdict | append verdict; acceptance controller rejects policy | `Failed`; source attempt stays `Succeeded` |
| advisory evaluator low verdict | append verdict/recommendation | none; task remains `Done` |
| evaluator launch/runtime/timeout/credential failure | evaluation job `Unavailable/FailedInfrastructure` | none; required task stays `AwaitingAcceptance` |
| evaluator evidence missing/corrupt/stale/wrong generation | rejected/unlinked evaluation evidence | none |
| human review rejection | acceptance evidence + `AcceptanceRejected` | `Failed`; source attempt stays `Succeeded` |
| ordinary/unrelated/post-terminal message | immutable message data | none |
| matching message for current wait | one `WaitSatisfied` receipt | `Waiting -> Open`, same generation |
| concurrent message read/delivery failure | delivery-side issue; immutable append retained | none |
| daemon crash at any side-effect boundary | replay/outbox/reconciliation | no inferred success or retry |
| legacy ambiguous history | `MigrationIssue`, fail-closed readiness hold | no guessed transition |

A source `Failed` by execution can still have advisory evaluations attached for diagnosis, but no verdict rescues it. Retry is a separate generation event.

## 9. Messages and waits

### 9.1 Messages are data

Message bodies, sender names, priorities, `Sent/Delivered/Read` observations, and `last_interaction_at` have **zero lifecycle authority**. Message activity may update a separate UI-only `last_message_at`; it is never a worker heartbeat or task-transition event. Appending or reading an ordinary message cannot:

* reopen or create a generation;
* keep an attempt/process alive;
* reset liveness timers;
* alter eligibility/readiness;
* satisfy dependencies;
* clear failure; or
* create a response child.

Post-completion follow-up is an explicit `wg retry`, `wg add --after`, or `wg msg follow-up --new-task` operator action. The coordinator never guesses relevance.

### 9.2 Correlated waits

A message can affect lifecycle only when the current generation is already:

```text
Waiting(Message {
  wait_id,
  correlation_id,
  accepted_senders,
  message_id_barrier,
  expires_at?,
})
```

The wait matcher uses WG receipt order/message IDs, not sender wall-clock time. It finds a message newer than the barrier that matches the correlation and sender policy, then requests `WaitSatisfied(wait_id, message_id, generation)`. The transition atomically records a unique wait receipt and moves `Waiting -> Open`. Replaying the tick, rereading the message, or restarting cannot satisfy it twice.

Unrelated messages remain readable data. A message matching an old generation or replaced wait is stale evidence only. Human input uses the same mechanism with an explicit human sender policy; it cannot directly complete a task.

### 9.3 Message durability

The message JSONL is immutable append-only data. Per-consumer delivery/read/ack observations move to a delivery journal or sidecar keyed by `(message_id, consumer_id)`. Status bookkeeping never rewrites the message log. Cursor and delivery updates do not confer lifecycle authority. Append acknowledgement requires durable storage; a failed delivery write cannot remove an accepted message.

## 10. Lazy evaluation and acceptance

### 10.1 Records, not eager task satellites

Publishing a source task no longer creates `.evaluate-*`, `.flip-*`, or verifier task rows. When source success or an explicit `wg evaluate run` makes evaluation relevant, the kernel lazily creates an `EvaluationRecord`:

```text
evaluation_id, source_task, generation, source_attempt
policy_snapshot, route_snapshot, threshold/quorum
state: Queued | Running | EvidenceAvailable | Unavailable | Cancelled
runner_attempts[], evidence_ids[], created_by_event
```

Evaluator runner attempts are not source attempts. Their process failures are recorded inside the evaluation record. Verdicts are immutable, content-addressed evidence bound to the exact source tuple, policy, route, and evaluator identity.

For compatibility and observability, `wg show --internal` may render records as **virtual satellites** with stable aliases such as `.evaluate-X@generation`. They are projections, have no task status, own no dependency edges, and cannot be messaged/retried through ordinary task lifecycle commands. A temporary compatibility materializer may create legacy satellite rows, but those rows are explicitly non-authoritative and their terminal status never implies a verdict.

### 10.2 Policy

Evaluation policy is pinned per generation:

* **None:** no evaluation record unless requested later; acceptance ignores evaluation.
* **Advisory (default when optional evaluation is enabled):** all non-evaluation hard gates can accept the task immediately. Evaluation runs lazily. Its result may recommend a new task/retry but never changes the current generation.
* **Required:** source success moves to `AwaitingAcceptance`. Exact valid evidence satisfying the pinned threshold/quorum permits `AcceptanceSatisfied`; an exact semantic reject permits `AcceptanceRejected`. Missing or infrastructure-failed evidence leaves the task awaiting and exposes an operator action.
* **Manual:** a typed human approval/rejection record is the hard gate. This replaces `PendingValidation`.

Evaluator infrastructure retries follow a separate evaluation retry policy and budget. They never increment source retry/launch breakers. Operators may explicitly retry evaluation, reject, or waive a required gate with an audited waiver permitted by policy. A waiver is acceptance evidence, not silent promotion.

### 10.3 No rescue authority

Low scores, evaluator failure, late evidence, and FLIP recommendations cannot emit `GenerationCreated`. Bounded “rescue” becomes a recommendation plus, if configured, a request to the ordinary persisted retry controller. The controller must append `RetryAuthorized` against a terminal failed/rejected generation and consume the task's retry budget. The evaluation subsystem itself remains append-only evidence.

## 11. Completion and worktree protocol

The source-bearing happy path is:

1. The current handler finishes and the wrapper captures a content-addressed output/deliverable manifest.
2. The wrapper submits completion intent with task/generation/attempt/fence.
3. Graph-only preconditions are checked under the final lock; external receipts are digest-verified.
4. Once the handler is quiescent, the kernel commits `AttemptSucceeded`, changes the worktree lease `Active -> Sealed`, and moves the task to `AwaitingAcceptance`.
5. An idempotent merge outbox action operates on the sealed lease epoch.
6. The merge receipt is appended. Conflict becomes an explicit merge hold; success satisfies the merge part of acceptance.
7. Required evaluation/manual evidence is collected if configured.
8. The acceptance controller rechecks the pinned dependency revisions and policy under lock, commits `AcceptanceSatisfied`, moves the task to `Done`, and records worktree integration/release atomically.
9. Cleanup/archive/push actions may finish later and are visible. They cannot erase source or retroactively alter acceptance.

Non-source shell, human, imported, and remote results use typed acceptance adapters. Each adapter states which policy clauses it satisfies; none bypasses the kernel. Remote signed acceptance can replace a local merge receipt only when the pinned policy explicitly names that receipt kind.

Resetting an accepted upstream does not silently invalidate a terminal downstream. The reset command must preview and explicitly include the affected closure or record a taint/override. Completion always rechecks dependency revisions before acceptance.

## 12. Daemon restart, replay, and reconciliation

### 12.1 Startup order

Every daemon start and every maintenance tick performs the same idempotent phases:

1. acquire the coordinator observation lease if configured; lifecycle correctness still relies on event CAS, not one daemon assumption;
2. scan/checksum the ledger and replay from the projection's ledger head;
3. verify projection invariants; create a `ProjectionMismatch` issue rather than guessing if replay cannot explain the snapshot;
4. ingest durable outbox receipts and evaluation/merge evidence by content ID;
5. reconcile process records using PID start identity/nonce;
6. reconcile worktree metadata/physical paths against lease epoch;
7. match correlated waits against immutable messages;
8. resolve completed reconciliation issues whose proof is now present;
9. evaluate explicit retry/cron/cycle policies and append uniquely keyed requests; and
10. compute readiness/admission and reserve new attempts.

No phase writes status directly. Each phase proposes events with deterministic idempotency keys, for example `lost:<attempt>:<process_nonce>` or `wait:<wait_id>:<message_id>`.

### 12.2 Crash convergence

| Crash boundary | Replay result |
|---|---|
| before attempt reservation event | task remains `Open`; unreferenced preparation is garbage-collected by action ID |
| after reservation, before process spawn | attempt remains `Preparing`; outbox resumes or a single launch attempt failure is recorded |
| after process spawn, before permit | gated process cannot execute; reconciliation cancels/fails the one attempt |
| just after permit | graph attempt/process identity is sufficient to adopt the live worker |
| after process exit, before attempt terminal event | exact observation requests one failure/lost event unless a completion intent exists |
| after attempt terminal, before process update | process observation is repaired; it cannot contradict first-terminal state |
| after merge, before receipt link | content-addressed receipt is linked on replay; merge action is idempotent |
| after verdict write, before link | exact verdict is linked on replay; stale verdict remains historical |
| after ledger append, before graph projection | projector applies the committed event |
| after projection, before ancillary archive | outbox resumes; semantic state is unchanged |
| during reset/cancel | task remains held until fence receipt; never advertised ready with stale owner |

Repair reaches one of: valid `Open`, valid current attempt, explicit `Waiting`, explicit `AwaitingAcceptance`, terminal generation, or an operator-required issue. “Spawned 0” without an exact gate is not an acceptable terminal diagnosis.

### 12.3 Exact ownership

* The lifecycle ledger owns logical task/attempt authority.
* The current fence token owns the right to propose attempt transitions.
* The worktree lease epoch owns physical workspace mutation.
* The wrapper owns child process signaling, but process existence grants no task authority.
* The dispatcher owns admission/reservation requests, not task success/failure.
* The acceptance controller owns policy evaluation, not source execution.
* No daemon instance owns task state in memory.

Multiple reconcilers may observe the same fact; event idempotency and CAS make one result authoritative.

## 13. Repair, rescue, and scaffolding disposition

### 13.1 Retain as automatic, idempotent reconciliation

* ledger replay and materialized-projection rebuild;
* permit-gated spawn and ownership-checked cancellation;
* exact dead-process observation to one failed/lost attempt;
* terminal zombie process reaping;
* content-addressed merge/verdict receipt linking;
* correlated wait matching;
* stale delivery/outbox retry;
* fail-closed worktree preservation;
* exact retry/cron/cycle policy evaluation; and
* derived readiness and breaker metrics.

These mechanisms restore already-decided facts or execute a persisted policy. They do not infer success or relevance.

### 13.2 Remove

* content-blind `Sent`-message resurrection and response-child creation;
* triage/model verdict authority to mark `Done`;
* non-human `PendingValidation` production and next-tick promotion;
* evaluator/FLIP direct rescue or reopen authority;
* eager evaluation/verification task scaffolding;
* stuck-`Blocked` status rewriting;
* whole-file mutable message delivery status;
* duplicate retry/requeue status writes;
* spawn-breaker charges for stale claims/worktree ownership conflicts; and
* any specialized helper's direct `Done`, `Failed`, or `Open` assignment.

### 13.3 Retain only as explicit operator commands/presets

Existing command names remain safe presets over one recovery engine:

* `wg retry`: new generation; default worktree reuse after fence; explicit `--fresh` allocates new and retains old.
* `wg requeue`: compatibility alias for an explicit new generation only from a terminal/held generation; never clears a live owner.
* `wg reset`: previews closure, cancels/fences live attempts, then creates generations; no immediate raw status rewrite.
* `wg recover`: dry-run batch of the same requests; does not invent completion.
* `wg replay`/archive restore: imported generation with provenance.
* `wg worktree adopt|release|archive|quarantine`: resolves explicit lease issues.
* `wg lifecycle mark-dead`: operator proof for ambiguous process identity.
* `wg accept --override`, `--accept-broken-deps`, or `--waive-evaluation`: named policy-controlled evidence, never an implicit manual bypass.
* `wg msg follow-up --new-task`: turns post-terminal discussion into explicit work.

Scaffolds for assignment, FLIP, or evaluation become lazy job records. If assignment must block execution, it is a pinned readiness policy/job result, not a hidden task status writer.

## 14. Backward-compatible migration

### 14.1 No flag day

Migration proceeds while old graph rows remain readable:

1. Add ledger/projection metadata and serde-defaulted generation/attempt/fence fields.
2. On first authoritative access, append one `LegacyCheckpointImported` event containing the raw task-row digest, mapped state, provenance, and confidence. Do not rewrite historical lines.
3. Run the kernel in shadow mode and compare projected results with legacy writers.
4. Convert command families to requests one at a time behind existing CLI names.
5. Cut over status writes to the kernel and make direct writes a CI/build violation.
6. Retain compatibility status rendering for one deprecation cycle.
7. Remove repair/scaffold writers only after diagnostics show no unexplained legacy mutations.

During dual-read, the ledger head plus graph lifecycle revision decides authority. A graph change not explained by the ledger creates `LegacyDirectMutationObserved`; the compatibility importer may checkpoint it only before cutover. After cutover it is an invariant violation and readiness holds fail closed.

### 14.2 Status mapping

| Legacy shape | Canonical import |
|---|---|
| `Open`, unassigned | generation 0 `Open` |
| `Open`, assigned | `Open` plus `MigrationIssue(StaleOwnership)`; not ready until reconciled |
| `InProgress` + exact live registry/process | synthetic current attempt `Running` with a minted fence |
| `InProgress` + conclusively dead process | synthetic attempt `Lost`; generation `Failed` |
| `InProgress` with ambiguous identity | preserved projection plus operator-required migration hold; no guessed reopen/failure |
| `Waiting` | `Waiting`; synthesize a wait ID and immutable message-ID barrier |
| `Done` | terminal `Done` plus `LegacyAcceptance` recording which modern evidence is unknown |
| `Failed` | terminal `Failed` plus legacy failure record; synthesize attempt only when ownership is provable |
| `Abandoned` | terminal `Abandoned` with compatibility `satisfy_dependents=true` |
| `Blocked` | generation `Open` plus derived dependency/policy issue; no canonical `Blocked` |
| `Incomplete` | terminal `Failed(IncompleteLegacy)` eligible for explicit/pinned retry |
| `PendingEval` | `AwaitingAcceptance` with imported required policy/evaluation record bound to mapped generation |
| `FailedPendingEval` | source generation `Failed`; legacy evaluation continues advisory and cannot rescue/reopen |
| `PendingValidation` + human-review | `AwaitingAcceptance` with manual policy |
| other `PendingValidation` | `Done` plus explicit `LegacyValidationMigrated` acceptance, preserving current effective behavior without silent promotion |
| stale `completed_at` on nonterminal | retained in legacy event evidence, omitted from normalized current projection |

Old evaluation `source_attempt` and `pipeline_id` become aliases to the imported generation/source attempt. Exact old verdicts retain their identity. Unknown mappings are never accepted as exact required evidence.

### 14.3 Legacy messages and waits

Old messages import as immutable data plus delivery observations. No old `Sent` message receives wake authority. For a task already in legacy `Waiting(Message)`, migration snapshots the current maximum message ID as its barrier and creates a one-use `LegacyAfterBarrier` correlation. Only a newer allowed message can satisfy that explicit existing wait. Operators may replace it with a strict correlation token.

### 14.4 Compatibility commands and output

`wg done/fail/wait/retry/requeue/reset/recover/approve/reject` retain their names but submit typed requests. Scripts receive stable rejection codes and a compatibility human message. `Pending*`, `Blocked`, and `Incomplete` may remain accepted input spellings/import renderings during the deprecation window, but new events never produce them.

## 15. Operator-visible diagnostics

### 15.1 `wg why-not-ready TASK`

Dispatcher and command call the same pure ordered gates. Default output reports the first effective gate; `--all` reports all in order. JSON includes:

```json
{
  "task": "synth-middle",
  "generation": 4,
  "task_state": "open",
  "ready": false,
  "stage": "ownership",
  "gate": "reconciliation.worktree-conflict",
  "reason_code": "worktree_lease_epoch_mismatch",
  "subject": {"issue_id":"ri_123","path":"...","expected_epoch":8,"found_epoch":7},
  "origin_event_id": "ev_456",
  "automatic_release": null,
  "suggested_command": "wg worktree inspect synth-middle",
  "graph_revision": 91,
  "ledger_head": 844,
  "config_generation": 12
}
```

Exit codes are `0=ready now`, `2=deterministically held/deferred`, and `3=invariant evidence unavailable`. Diagnosis is read-only and never increments counters.

### 15.2 Lifecycle views

* `wg lifecycle show TASK`: generations, accepted events, current fence, acceptance record, and late/rejected evidence.
* `wg attempts TASK`: attempts, process observations, route, cost, terminal disposition, and derived retry/breaker counts.
* `wg worktree status TASK`: lease epoch/holder/state, merge receipt/conflict, physical verification, and safe commands.
* `wg evaluate status TASK`: policy, lazy jobs, runner attempts, evidence validity, and hard/advisory effect.
* `wg reconcile status [TASK]`: open/deduplicated issues, first/last seen, readiness effect, and outbox attempts.
* `wg lifecycle doctor`: projection/ledger mismatch, `Done` without acceptance, active attempt/fence mismatch, stale lease, and legacy ambiguity.

Daemon tick output reports counts by exact gate and event reason, not only `spawned=0`. Every transition line includes event, task, generation, attempt, actor, old/new state, reason, and evidence IDs.

### 15.3 Loop detector

The daemon records a bounded signature of `(task, generation, state, first_gate, current_attempt, lease_epoch, issue_id)`. Repeated identical signatures with attempted mutations trip `transition_loop_detected`, suppress further automatic requests for that signature, and expose the originating events. Observation-only ticks do not count as loops.

Metrics alert on:

* rejected stale-attempt terminal reports;
* terminal message-mutation attempts (which should be zero after cutover);
* `Done` without acceptance;
* current attempt/fence mismatch;
* ownership issue charged as spawn failure (must be zero);
* duplicate reconciliation action;
* required evaluation unavailable duration; and
* repeated generation creation with one cause key.

## 16. Model-based transition conformance

### 16.1 Reference model

A pure reference model contains all independent domains, not one overloaded enum:

```rust
struct Model {
    task: TaskGeneration,
    attempts: BTreeMap<AttemptId, Attempt>,
    processes: BTreeMap<AttemptId, ProcessObservation>,
    admission: Vec<AdmissionDecision>,
    provider_health: BTreeMap<Route, ProviderHealth>,
    worktrees: BTreeMap<WorktreeId, WorktreeLease>,
    evaluations: BTreeMap<EvaluationId, EvaluationRecord>,
    waits: BTreeMap<WaitId, WaitReceipt>,
    issues: BTreeMap<IssueId, ReconciliationIssue>,
    ledger_ids: BTreeSet<EventId>,
}
```

Generated valid/invalid requests are applied both to the model and a disposable real graph. After each action and optional restart, compare semantic projections and structured rejection codes. The generator varies actor, idempotency key, revisions, generations, attempts, fences, dependency changes, process order, message delivery, worktree lease epochs, evaluation evidence, and crash points.

### 16.2 Required properties

1. Only one attempt is current and only its fence can terminalize or park it.
2. First terminal attempt disposition wins under every report ordering.
3. Every terminal generation is immutable; only a greater explicit generation runs later.
4. Every `Done` has a policy-valid acceptance record.
5. No arbitrary message sequence changes state, liveness, readiness, generation, or breaker counts.
6. A current correlated wait consumes one matching message at most once across restarts.
7. Admission deferral creates no attempt/failure/cost.
8. Every post-reservation process launch/runtime failure creates exactly one failed attempt.
9. Evaluator infrastructure failure never changes source attempt/task failure state.
10. Required verdict freshness is exact; advisory evidence never changes task state.
11. Low required evidence can reject but never reopen; retry always has a separate authorization/generation.
12. Worktree transfer/release and motivating event share one commit/fence.
13. No old process can mutate after reset/retry/reassignment.
14. Ownership/reset conflict is one deduplicated, breaker-neutral issue.
15. Reset cannot become ready before a fence or safe fresh-worktree decision.
16. Reconciliation and outbox processing are idempotent.
17. Crash at every ledger/projection/process/merge/evaluation/message boundary converges.
18. Source-bearing work is never automatically deleted.
19. Dispatcher and `why-not-ready` return the same first gate for one snapshot.
20. Derived budgets equal attempt/evaluation records and cannot be double-charged.

### 16.3 Deterministic interleavings

Permanent tests place barriers:

* completion intent before/after reset and reassignment;
* dependency check before an upstream new generation;
* reservation, process spawn, registry receipt, and launch permit;
* process exit before/after completion intent;
* worktree seal, merge, receipt, and acceptance;
* message append, per-consumer delivery, wait match, and receipt;
* verdict write/link before/after retry/new generation; and
* reset cancellation before/after process and worktree fencing.

The full survey nightmare trace is a table-driven model case with a restart between every adjacent pair. Expected result: late done is stale evidence; evaluation cannot reopen; the unrelated message is inert; stale worktree ownership creates one neutral issue; reset remains held until fenced; and the loop detector prevents repeated mutation.

### 16.4 Static authority check

CI scans production Rust for task status assignment/construction. The allowlist contains only:

* the lifecycle projector/kernel;
* deserialization of compatibility snapshots; and
* annotated migration fixtures.

The direct-writer inventory in survey §3.3 is the burn-down list. New public helpers accepting a raw `Status` fail review. This check permanently enforces the promised reduction to one authority.

## 17. Staged implementation

### Stage 0 — pin behavior with tests

Add the reference model skeleton, authority scan, message non-resurrection reproducer, stale completion race, ownership-conflict neutrality, and restart fault points. Existing tests selected for removed behavior are explicitly inverted rather than silently deleted.

### Stage 1 — ledger and fenced core

Add serde-defaulted generation/attempt/fence/revision fields, ledger/checkpoint import, the kernel, compatibility projections, and request wrappers. Convert claim/spawn/done/fail/wait first. Require acceptance records for new `Done` events. Triage loses completion authority.

### Stage 2 — ownership, process, and worktree protocol

Move process observations and worktree leases behind fences/outbox. Convert reset/retry/recover. Make reset await quiescence and make ownership issues breaker-neutral. Preserve worktrees by default.

### Stage 3 — messages

Make message bodies immutable, add per-consumer delivery records and correlated waits, and delete resurrection. Import legacy waits with a barrier. Add append/read concurrency stress.

### Stage 4 — lazy acceptance/evaluation

Create evaluation records on demand, render virtual satellites, stop new eager scaffolds, remove non-human `PendingValidation`, and separate evaluation-runner retries from source attempts. Keep required legacy policies through imported records.

### Stage 5 — remaining writers and status retirement

Convert cron, cycles, remote exec, chat/human, matrix/agent helpers, import/archive, and specialized commands. Stop producing `Blocked`, `Incomplete`, and all `Pending*` statuses. Enable the static authority check as a hard gate.

### Stage 6 — remove compatibility repair

After at least one release of diagnostics shows no unexplained direct mutation, delete legacy resurrection, stuck-block rewrite, rescue, eager satellite, and projection-import code that is no longer needed. Keep ledger replay, outbox repair, worktree preservation, and operator recovery permanently.

No stage requires rewriting the entire graph. Each task is checkpoint-imported lazily, and existing command names remain adapters throughout.

## 18. Acceptance brief: permanent scenarios

The implementation is not complete until these credential-free scenarios are in `tests/smoke/scenarios/` and permanently owned in the grow-only manifest, plus the model/property suite runs in CI:

1. **`lifecycle_stale_terminal_fence`** — A starts completion; operator resets/fences; B owns the next generation; A's completion/failure is evidence only and B remains owner.
2. **`lifecycle_first_terminal_wins`** — all pairwise orderings of success/failure/lost/park for one attempt; exactly the first changes state.
3. **`lifecycle_message_is_data`** — an irrelevant `Sent` message to `Done`, `Failed`, `Open`, and `Running` tasks changes no lifecycle/liveness/readiness fact, including after repeated ticks/restarts.
4. **`lifecycle_correlated_wait_once`** — unrelated message does not wake; matching message wakes the existing wait once across restart; replay is inert.
5. **`lifecycle_admission_deferral`** — capacity/disk/provider backpressure creates no attempt or breaker charge and dispatches when the fresh gate clears.
6. **`lifecycle_process_failure_attempt`** — launch error, nonzero exit, signal, zero-output, and lost process each create one typed failed attempt; pinned retry creates one greater generation.
7. **`lifecycle_worktree_fenced_transfer`** — retry-in-place preserves source and atomically transfers only after quiescence; fresh retry retains the old tree; conflicts are neutral issues.
8. **`lifecycle_reset_awaits_fence`** — reset during live execution never advertises ready or reuses its worktree before process/worktree fence proof.
9. **`lifecycle_completion_acceptance`** — every local/shell/human/remote `Done` path has a typed policy-valid acceptance; missing deliverable/unmerged work/changed dependency refuses it.
10. **`lifecycle_lazy_evaluation`** — no eager source satellites; advisory pass/fail leaves `Done`; required pass accepts; required low rejects without reopening; evaluator crash leaves source success and task awaiting.
11. **`lifecycle_stale_evaluation`** — verdict for generation N after N+1 exists remains historical and cannot be consumed.
12. **`lifecycle_reconcile_restart_matrix`** — restart at each reservation/permit/process/ledger/projection/merge/verdict/outbox boundary converges idempotently.
13. **`lifecycle_message_concurrency`** — concurrent append/read/ack retains every message and isolates consumer observations.
14. **`lifecycle_legacy_migration`** — table in §14.2 maps deterministically; ambiguous ownership holds loudly; old `Sent` data never gains wake authority.
15. **`lifecycle_why_not_ready_equivalence`** — every readiness/admission/config/ownership gate matches dispatcher behavior and names its origin.
16. **`lifecycle_composite_nightmare`** — exact cross-graph trace from survey §10, with a restart between every hop; no second loop or charged ownership failure.
17. **`lifecycle_cycle_cron_generations`** — due schedule/iteration creates one uniquely keyed greater generation; duplicate tick does nothing; stale eval cannot cross.
18. **`lifecycle_authority_scan`** — no direct task-status writer outside kernel/projector/migration allowlist.

The model-based suite additionally randomizes at least the properties in §16.2 and shrinks failures to a minimal event sequence. Fault tests cover `EIO`, `ENOSPC`, kill/restart, partial final ledger frame, and idempotent external receipts.

### 18.1 Release criteria

A release may claim the simplified lifecycle only when:

* the authority scan reports one status writer;
* the complete legacy writer burn-down is empty;
* all 18 permanent scenarios pass;
* the model reports no state divergence over the configured randomized run budget;
* every `Done` created after cutover has an acceptance record;
* ordinary message terminal mutations and ownership-conflict breaker charges are zero; and
* `wg lifecycle doctor` reports no unexplained projection mismatch on migration fixtures.

## 19. Rationale and rejected alternatives

### 19.1 Why generations instead of reopening

WG needs retry, cron, cycles, and replay. Prohibiting later work entirely would remove useful behavior; mutating terminal state loses history and permits stale evidence. Numbered generations preserve both. “Reopen” survives only as CLI vocabulary for an explicit new generation, never as a state edge on terminal history.

### 19.2 Why an event ledger plus projection

A central setter alone would reduce writers but would not make crash replay, origin diagnostics, idempotency, or historical ambiguity deterministic. A ledger alone without one kernel would still permit competing semantics. The combination is required. SQLite could implement it later, but is not necessary for the behavioral cutover.

### 19.3 Why `AwaitingAcceptance`

A source attempt can succeed while merge, required review, or approval remains unresolved. Calling that `Running` lies about the worker, and calling it `PendingEval` encodes one evidence type as task state. The generic acceptance state cleanly separates source outcome from policy outcome.

### 19.4 Why evaluator reject may fail a task but evaluator failure may not

A valid required verdict is semantic evidence evaluated by the task's pinned acceptance policy. An evaluator crash is infrastructure evidence about the evaluator only. The former may reject acceptance; the latter cannot rewrite source execution and leaves an explicit hard-gate hold.

### 19.5 Why reconciliation issues are not `Blocked` or spawn failures

Ownership and cross-domain ambiguity require attention, but neither says the task implementation failed. A dedicated deduplicated issue is self-diagnosing, can hold readiness without lying about task state, and cannot amplify into repeated spend/breaker trips.

### 19.6 Why post-terminal messages do nothing automatically

WG cannot infer whether free-form text requests correction, starts a new deliverable, acknowledges completion, or is irrelevant. Automatic relevance inference created the reproduced repeated resurrection. Explicit follow-up work is cheaper than an unsafe hidden transition authority.

## 20. Final ratification

The durable defenses identified by the survey remain: atomic graph projection, permit-gated launch, immutable exact verdicts, source-preserving worktrees, retry-in-place, explicit waits, and idempotent reconciliation. Their role changes. They provide evidence or execute ledger decisions; they do not write task state independently.

The authoritative rule is simple:

> An actor may submit evidence and a typed request. Only the lifecycle kernel, under the ledger lock and the current generation/attempt/worktree fences, may change lifecycle state.

That rule permanently removes message resurrection, stale completion, evaluation rescue, alternate triage completion, and ownership-breaker loops while retaining deliberate recovery and repeated work as explicit, auditable generations.
