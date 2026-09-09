# Minimal bounded source-provider recovery contract

**Status:** accepted V1 design for `provider-backoff-planner` and its direct
real-entry-point verification

## 1. Decision and scope

V1 adds a small, explicitly enabled recovery path to the **current direct
source dispatcher**. It automatically retries only an unambiguously classified,
transient **source-provider** failure:

- direct HTTP 429 / provider rate limit;
- a direct temporary server/provider outage such as eligible 5xx, overloaded,
  or unavailable; or
- a typed transport failure whose execution outcome is proven safe to replay or
  has been reconciled as not committed.

The default is off:

```toml
[coordinator.source_provider_retry]
enabled = false
```

With the policy off, the existing fail-stop behavior is unchanged: the exact
source attempt remains `Failed`, its worktree and evidence remain available,
and only an explicit operator action can create another generation. Enabling
the policy is prospective; it does not backfill old failures.

The V1 budget is deliberately small:

- at most **3 automatic retries**;
- all automatic retry starts must occur within **15 minutes of the first
  failure** in the episode;
- deterministic exponential delay starts at 30 seconds and is capped at 5
  minutes; and
- provider `Retry-After` is a lower bound. If it falls after the episode
  deadline, recovery becomes `NeedsAttention` rather than retrying early.

Attempt count and elapsed time are independent budgets. The delay cap does not
create more attempts and reaching it is not semantic failure. Exhausting either
budget yields a visible `NeedsAttention / recovery-exhausted` projection with
saved work and one safe next action; it does not assign a quality score or claim
the source is bad.

V1 does **not** add evaluator/reviewer retries, Agency learning/projection,
hour/day unattended recovery, route-wide probe/breaker architecture, rich TUI
controls, a new scheduler, or an LLM controller. Those are deferred explicitly
in section 11.

## 2. Current authority: extend it, do not replace it

### 2.1 Production dispatcher

The deployed service announces:

```text
Direct fail-stop dispatch enabled; PlannerStore is not an authority
```

and reports `dispatch_authority = "direct-fail-stop"`
(`src/commands/service/mod.rs:3259`, `:4804`). `coordinator_tick` does not open,
replay, acknowledge, or migrate planner effects. It derives ordinary work from
graph readiness and capacity, resolves one canonical `SpawnPlan`, and invokes
`spawn_agent_with_binding` with exact route and plan bindings
(`src/commands/service/coordinator.rs:2243-2289`). Global outage/backoff
controllers are retired (`src/commands/service/coordinator.rs:2874`).

A route-selection or process-launch error is terminalized by
`record_direct_dispatch_failure`; it says that the coordinator will not retry
implicitly (`src/commands/service/coordinator.rs:1835-1929`). A running source
provider failure flows through raw-stream terminal classification, `wg fail`,
and the lifecycle `AttemptFailed` transition (`src/commands/spawn/raw_stream_classifier.rs:16-177`,
`src/commands/fail.rs:93-278`). A `Failed` task is not ordinary ready work.

V1 adds one more **admission case inside that direct dispatcher**: a failed task
with a due, valid `SourceProviderRecoveryV1` record may receive one fenced
`GenerationCreated` transition and then use the same direct spawn path. There
is no background retry thread and no generic effect planner. Existing
coordinator capacity, task ordering, graph locking, claim/reservation, and
process launch remain authoritative.

### 2.2 Lifecycle and completion authority

`GenerationCreated` already requires an Operator or Reconciler, increments the
generation, clears the current attempt, and returns the task to `Open`
(`src/lifecycle.rs:959-982`). V1 requests this existing transition with exact
expectations; it never writes source status directly.

The completion controller remains the sole ordinary authority which derives
`Done` from immutable candidate bytes, deterministic validation, exact review
receipts, and publication truth (`src/commands/completion_done.rs:75-238`). A
selected completion candidate, completion blocker, review rejection,
publication refusal, or integrity error is not a source-provider retry signal.
The current raw-stream precedence already protects completed turns and typed
finalization blockers from being rewritten as heuristic provider failures
(`src/commands/spawn/raw_stream_classifier.rs:16-177`). V1 preserves and
strengthens that precedence.

A late result from a superseded source attempt remains fenced by task,
generation, attempt ID, owner, and fence. Provider recovery receives no
publication or completion capability.

### 2.3 Typed failure and exact route seams

`FailureSignal` currently carries a normalized reason, confidence, HTTP status,
provider code/type, relative Retry-After, executor, route, and detection time
(`src/graph.rs:305-323`). Shared telemetry maps structured status/provider data
before falling back to text (`src/telemetry/mod.rs:1-299`). Rolling telemetry
and `ProviderHealth.cooled_until_ms` are diagnostic projections only
(`src/telemetry/mod.rs:311-337`, `:447-503`); they are not restart-stable retry
authority.

The direct spawn path already computes a redacted `HealthRouteKey`, a stable
route binding, and a complete plan binding (`src/service/provider_health.rs:25-84`,
`src/dispatch/plan.rs:344-385`). `spawn_agent_with_binding` recomputes the plan
and refuses mismatch without fallback (`src/commands/spawn/execution.rs:1204`,
`:1325-1338`). V1 persists and reuses those bindings.

### 2.4 Authorities which remain inactive

`src/service/planner.rs`, `src/service/convergence.rs`, and
`[coordinator.convergence]` still compile, but production never opens
`PlannerStore`. Comments in those inactive modules are not dispatch authority.
V1 must not:

- open, migrate, or replay `PlannerStore`;
- revive `ConvergenceState` or `converge_failed_prerequisites`;
- create a general retry/effect store or a second scheduler;
- use telemetry cooldown as an admission clock;
- recreate `.evaluate-*`, `.flip-*`, probe, retry, or controller graph tasks; or
- invoke a model to classify a provider error.

## 3. V1 persistence on the existing task record

V1 reuses the atomically persisted graph. It adds one optional projection to
`Task`, serialized in `graph.jsonl` and mutated in the same `modify_graph`
transaction as the relevant lifecycle event:

```text
SourceProviderRecoveryV1 {
  schema: 1,
  episode_id,
  state: Backoff | Authorized | Running | Recovered |
         NeedsAttention | Paused | Cancelled,

  goal_requirements_digest,
  completion_contract,
  origin: SourceAttemptRef,
  last_failed: SourceAttemptRef,
  current_failure_id,
  failure_evidence_digest,
  execution_outcome: DefinitiveFailure | NotSent | ReconciledNoCommit,

  exact_route,                 // redacted handler-first route
  route_id,
  plan_id,

  first_failure_at,
  latest_failure_at,
  recovery_deadline_at,
  retry_after_not_before,
  automatic_retries_used,
  automatic_retry_limit,
  next_retry_at,
  policy_snapshot,

  authorization_id,
  authorized_generation,
  reason_code,
  next_action,
}
```

`SourceAttemptRef` contains graph/task ID, generation, attempt ID, attempt fence,
owner/run identity, process epoch where applicable, and the lifecycle revision
which accepted `AttemptFailed`.

The episode record is retry admission state, not a second task lifecycle. The
task remains canonically `Failed` while in Backoff or NeedsAttention. Human
views render the typed recovery substate so `Failed` is not misread as a
semantic quality judgment.

The stable episode identity is:

```text
episode_id = BLAKE3(
  "wg-source-provider-recovery-episode-v1" || graph_id || task_id ||
  goal_requirements_digest || completion_contract || route_id || plan_id ||
  first_failure_id
)
```

The existing graph writer lock and atomic save make deadlines, counters, route
bindings, authorization, and lifecycle change restart-stable together. No new
retry file, timer service, journal, planner, or route probe store is introduced.
The coordinator's existing periodic/event tick checks `next_retry_at`.

The task lifecycle audit receives stable recovery event IDs for durable history.
Closed episode details may later be compacted from the projection only after the
same episode/authorization IDs are present in lifecycle audit and attempt
metadata; no active Backoff, Authorized, Running, Paused, or NeedsAttention
record may be discarded.

## 4. Eligibility and decision table

An automatic retry requires every positive condition in the table and all
fences in section 7. Direct evidence means a structured HTTP response/provider
envelope or a typed transport result from the execution adapter—not a phrase in
stderr and not model inference.

| Failure evidence and state | V1 classification | Automatic source action | Visible result / next authority |
|---|---|---|---|
| Direct 429 or provider rate-limit envelope; execution definitively rejected | `transient-source-provider` | Schedule within both budgets; honor Retry-After | Backoff, then one exact-route source retry |
| Direct eligible 500/502/503/504/529 or typed provider overloaded/unavailable | `transient-source-provider` | Schedule within both budgets | Backoff, then one exact-route source retry |
| Typed connect/DNS failure before request bytes were sent | `transient-source-transport` | Schedule within both budgets | Backoff, then one exact-route source retry |
| Typed reset/timeout after request start, with provider idempotency or reconciliation proving no committed execution/result | `transient-source-transport` | Schedule within both budgets | Backoff, then one exact-route source retry |
| Reset/timeout where provider acceptance, tool effects, or final result may have occurred and are not reconciled | `ambiguous-execution` | **Never** | NeedsAttention; reconcile the exact run before any explicit retry |
| Text-only “429”, “timeout”, “unavailable”, generic nonzero exit, contradictory direct reports, or unknown reason | `unknown` | **Never** | Preserve evidence; operator inspection/reconciliation |
| Whole-agent hard timeout without nested direct provider evidence | `source-timeout` | **Never** under this policy | Existing source/operator recovery policy |
| 401/403, missing/invalid key, missing handler, invalid endpoint/model, route/config drift | `auth-config` | **Never** | Fix/authenticate configuration, then explicit operator retry |
| 402, insufficient credits, account/project budget exhausted | `credit-exhausted` | **Never** | Add credit/raise budget, then explicit operator retry |
| Input/document 4xx, context/token limit, bad request | `source-input` | **Never** | Correct source/configuration explicitly |
| Semantic validation failure or FLIP/Eval/reviewer rejection | `semantic-rejection` | **Never** | Existing exact-candidate repair/waiver path; elapsed time is inert |
| Reviewer/evaluator provider or process failure | out of V1 | **Never by source recovery** | Existing reviewer/evaluator policy; unchanged source |
| Completion-controller, evidence-integrity, publication, landing, or guard failure | `completion-authority` | **Never** | Existing retained-candidate completion recovery |
| Task is manually paused, cancelled, abandoned, or has a newer operator generation | `operator-state` | **Never while/after that state** | Manual authority is preserved |

A Retry-After value does not make a 401/402/403 retryable. Conversely, an
eligible class without safe execution-state evidence is ambiguous, not
transient. The implementation uses direct typed evidence whenever available and
never asks an evaluator to rediscover it.

## 5. Direct evidence, safe replay, and deduplication

### 5.1 Evidence additions

The shared source failure boundary must add the minimum fields currently
missing from `FailureSignal`/attempt metadata:

```text
FailureEvidenceKind = HttpResponse | ProviderEnvelope | TransportError |
                      ProcessOutcome | LegacyText | Unknown

SourceProviderFailureEvidence {
  evidence_kind,
  operation_id,
  provider_request_id,
  route_id,
  plan_id,
  exact_route,
  http_status,
  provider_type_or_code,
  transport_code,
  retry_after_not_before,
  execution_outcome,
  evidence_digest,
}
```

`operation_id` is minted before physical source execution; the current durable
spawn/run identity may be used when bound to the exact attempt. Provider request
IDs are aliases learned later. Attempt metadata must add `plan_id` and the
stable recovery authorization ID; the existing health route and lifecycle tuple
are retained.

An HTTP/provider response which definitively refused work is safe. A transport
failure is safe only when the adapter proves `NotSent` or persists a
`ReconciledNoCommit` result using provider idempotency/request identity. “No
graph result appeared” and “the process died” are not proof. Any possible
external tool or publication effect makes the outcome ambiguous until
reconciled.

### 5.2 Failure identity and precedence

```text
failure_id = BLAKE3(
  "wg-source-provider-failure-v1" || graph_id || task_id || generation ||
  attempt_id || fence || operation_id || route_id || plan_id
)
```

Wrapper, `wg fail`, raw-stream, telemetry, and process-observer reports for that
operation fold under the same `failure_id`. Evidence selection precedence is:

```text
ProviderEnvelope > HttpResponse > TransportError > ProcessOutcome >
LegacyText > Unknown
```

The first accepted direct terminal observation fixes `latest_failure_at` for
that failure. Duplicate observations:

- do not create a new episode;
- do not increment `automatic_retries_used`;
- do not recompute jitter or replenish the elapsed window;
- do not move a deadline earlier; and
- may only attach stronger direct evidence or increase an authoritative
  Retry-After lower bound.

Conflicting direct hard/transient classifications move the episode to
`NeedsAttention(reason=ambiguous-execution)` and cancel an unstarted
authorization. A new physical retry always receives a new `operation_id` and,
if it fails before authoritative recovery, becomes the next unique failure in
the same episode.

### 5.3 Completion and late-result fences

Source recovery is ineligible once the failed attempt has selected an immutable
completion candidate or entered a typed completion/finalization blocker. The
completion authority wins over incidental provider-looking text.

When a retry generation is created, the old attempt/fence is superseded. Any
late `wg done`, manifest, provider terminal receipt, or publication request from
the old owner fails the existing generation/attempt/fence/owner checks. A
recovery record never copies a candidate forward, accepts review, or publishes.
Partial source work remains only in the retained managed worktree/branch for the
fresh worker attempt to inspect and continue.

## 6. Exact bounded delay and budgets

Each episode snapshots this V1 policy:

```text
max_automatic_retries = 3
recovery_window_seconds = 900
base_seconds = 30
delay_cap_seconds = 300
jitter_divisor = 4
```

`first_failure_at` is the integer Unix second when the first unique eligible
failure is accepted. It never changes within the episode:

```text
recovery_deadline_at = first_failure_at + recovery_window_seconds
```

Let `r = automatic_retries_used` before authorizing the next retry. The first
retry has `r = 0`:

```text
raw(r) = min(
  delay_cap_seconds,
  saturating_mul(base_seconds, 2^min(r, 63))
)

jitter_window(r) = floor(raw(r) / jitter_divisor)

jitter(r) = u64_le_first_8_bytes(BLAKE3(
  "wg-source-provider-retry-jitter-v1" || episode_id ||
  current_failure_id || r
)) mod (jitter_window(r) + 1)

computed_delay(r) = min(delay_cap_seconds, raw(r) + jitter(r))

candidate_retry_at = max(
  latest_failure_at + computed_delay(r),
  retry_after_not_before.unwrap_or(0)
)
```

All arithmetic is saturating and all timestamps/delays are integer seconds.
BLAKE3 byte order and inputs above are normative. With defaults, the three local
delays begin at 30, 60, and 120 seconds (plus deterministic positive jitter,
each capped at 300 seconds).

The next automatic retry is admitted only if all are true:

```text
automatic_retries_used < max_automatic_retries
candidate_retry_at <= recovery_deadline_at
coordinator_now >= candidate_retry_at
coordinator_now <= recovery_deadline_at
```

If the count is already 3, state becomes
`NeedsAttention(reason=automatic-retries-exhausted)`. If current time or
`candidate_retry_at` is later than the 15-minute deadline, state becomes
`NeedsAttention(reason=recovery-window-expired)`. If an authoritative
Retry-After specifically causes that result, use
`reason=retry-after-exceeds-window`. WG never violates the lower bound by
retrying early.

A retry which started within the window is not killed when the deadline passes.
It may finish. Success closes the episode; failure becomes NeedsAttention if no
budget remains. The elapsed window limits **starts**, while the existing worker
timeout governs a started attempt.

The delay cap is only a single-delay cap. The independent count and elapsed
window are the retry budgets.

### 6.1 Retry-After normalization

Only an actual response header or structured provider envelope supplies an
authoritative lower bound:

- a finite nonnegative delta becomes
  `failure_observed_at + ceil(delta_seconds)`;
- an HTTP date becomes its ceiling Unix second;
- malformed, negative, NaN, or infinite values are diagnostic and ignored;
- duplicates merge by `max(existing, incoming)`; and
- a prose-extracted number is never authoritative.

The normalized absolute timestamp is persisted. Restart never reparses a header
or shifts a delta relative to restart time.

## 7. Automatic transition and idempotency fences

### 7.1 Enrolling the first failure

The source attempt first terminalizes normally through `AttemptFailed` with the
exact current `FenceExpectation`. In the same graph transaction, the policy may
create an episode only when:

1. the policy is enabled;
2. table 4 classifies direct eligible evidence;
3. execution outcome is safe;
4. task, generation, attempt, fence, owner/run, goal/requirements, completion
   contract, route, and plan all bind;
5. no candidate, completion blocker, newer generation, manual pause/cancel, or
   reopen intent exists; and
6. this `failure_id` is not already folded.

An ineligible failure still persists its normal typed evidence but creates no
automatic deadline.

### 7.2 Authorizing one retry

The coordinator considers due recovery only when an ordinary agent slot is
available. Capacity delay does not consume an attempt, but if capacity pushes
current time past the episode deadline the episode needs attention.

Inside one `modify_graph` transaction it rechecks the entire enrollment tuple,
current state `Failed + Backoff`, budgets, pause/cancel state, and direct
evidence. It recomputes the current canonical `SpawnPlan` and requires the exact
persisted handler/model/reasoning/endpoint fingerprint, `route_id`, and
`plan_id`. Any drift is
`NeedsAttention(reason=exact-route-changed)`; automatic fallback is forbidden.

For retry number `q = automatic_retries_used + 1`:

```text
authorization_id = BLAKE3(
  "wg-source-provider-retry-authorization-v1" || episode_id ||
  current_failure_id || q
)

lifecycle idempotency key = "source-provider-retry:" + authorization_id
reason_code = "transient_source_provider_retry_due"
actor = Reconciler("source-provider-retry")
transition = GenerationCreated
expected = exact current FenceExpectation
```

The same graph save applies `GenerationCreated`, increments
`automatic_retries_used`, records `Authorized`, records the new generation, and
persists `authorization_id`. The count is consumed at lifecycle authorization,
not at a later log line; a crash cannot receive a free fourth generation.

The ordinary ready selector must admit an `Open` recovery generation only when
its `Authorized` record matches that exact generation and authorization, the
policy is still enabled, and coordinator time has not passed
`recovery_deadline_at`. The existing direct spawn path receives the persisted
route/plan binding and checks it again. Its claim/reservation and launch permit
remain the one physical ownership boundary. Attempt and agent metadata persist
`episode_id`, `authorization_id`, and exact route/plan before state becomes
`Running`.

### 7.3 Crash and duplicate matrix

- **Crash before graph save:** no lifecycle generation or consumed retry exists;
  the same due state can be reconsidered.
- **Crash after graph save, before spawn:** lifecycle audit contains the
  idempotency key and the task is the exact authorized Open generation. Restart
  may spawn that generation only if the policy is enabled and the original
  recovery deadline has not passed; otherwise it remains held and becomes
  NeedsAttention. It never calls `GenerationCreated` again.
- **Crash during spawn preparation before launch permit:** existing spawn
  rollback applies. The same authorization remains consumed and may reattempt
  preparation only while route, window, and state still validate. A hard
  configuration error moves to NeedsAttention.
- **Crash after claim/launch permit:** assignment, attempt metadata, and
  authorization prove Running. Restart must not spawn another owner.
- **Duplicate failure evidence:** same `failure_id`, no counter or deadline
  reset.
- **Late old-owner result:** rejected by generation/attempt/fence/owner checks.
- **New retry attempt fails:** its new operation/failure ID updates the same
  episode; schedule the next retry only if both budgets remain.
- **New retry attempt succeeds authoritatively:** close Recovered exactly once.

Neither `retry_count`, cycle failure restart, rescue logic, `max_retries`, nor a
legacy convergence effect may create an additional automatic generation for an
enrolled provider failure. In particular, `wg fail` currently calls
`evaluate_cycle_on_failure` (`src/commands/fail.rs:273`); eligible V1 provider
failures must bypass that duplicate restart. With the V1 policy disabled they
remain fail-stop; with it enabled only the episode authorization may reopen.

## 8. Restart, pause, cancellation, and reset

### 8.1 Restart stability

The graph record is authoritative. Daemon/coordinator restart:

- does not change `first_failure_at` or `recovery_deadline_at`;
- does not replenish retries;
- does not recompute jitter or Retry-After;
- does not treat partial output as success;
- does not recreate `GenerationCreated` after its idempotency event; and
- performs at most one overdue authorization, never one catch-up retry per
  missed interval.

If restart occurs after the deadline, an unstarted episode moves to
NeedsAttention without provider I/O.

### 8.2 Manual pause and cancellation

A task-level user pause or global dispatch pause blocks automatic authorization.
Pause does **not** freeze or extend the 15-minute window and does not reset
attempts. Unpausing resumes the same episode only if both budgets still permit;
otherwise it becomes NeedsAttention.

Cancellation/abandonment closes the episode as `Cancelled`. It never reopens
automatically. Runtime disabling of the policy behaves like a pause for an
unstarted Backoff/Authorized record: no new provider call is made and no budget
is reset. A call already past its launch permit is allowed to finish under its
existing lifecycle/timeout fence; disabling is not a kill operation.

### 8.3 What resets an episode

Only either of these resets/closes the episode:

1. **Authoritative successful recovery:** an exact terminal provider/agent
   success receipt bound to the current retry operation, or a completion
   candidate durably selected from that operation. This records `Recovered`.
2. **Explicit operator retry/reset:** the existing audited operator action
   intentionally closes the old episode. A later eligible provider failure may
   start a new episode from zero.

Incidental stream bytes, tokens, tool logs, heartbeats, PID liveness, claim,
spawn, generation creation, restart, config reload, elapsed time, duplicate
failure evidence, success on a different task/route, or unpause never reset an
episode.

A successful provider operation followed by semantic, validation, completion,
or publication failure closes provider recovery and follows that other
failure's existing authority. It does not spend the remaining provider retries.

## 9. Minimal truthful status and next action

`wg show` / `wg show --json` and ordinary service status join the task's embedded
record without mutating it:

```text
source_provider_recovery: {
  schema,
  state,
  reason_code,
  episode_id,
  failure_id,
  evidence_digest,
  exact_route,
  route_id,
  plan_id,
  attempts_used,
  attempts_limit,
  attempts_remaining,
  first_failure_at,
  recovery_deadline_at,
  next_retry_at,
  retry_after_not_before,
  next_action
}
```

Secrets and raw provider bodies are never printed. `next_retry_at` is present
only for Backoff. Authorized/Running identifies the exact retry number.
NeedsAttention omits a retry time and provides exactly one action.

Example Backoff output:

```text
Source recovery: Backoff (retry 2 of 3, exact route; no fallback)
Reason: direct provider HTTP 429; evidence=b3:...
Next retry: 2026-09-09T21:04:05Z (window ends 2026-09-09T21:15:00Z)
```

Example exhaustion output:

```text
Source recovery: NeedsAttention (recovery-exhausted; 3 of 3 retries used)
Saved work: retained; failure evidence=b3:...
Next action: inspect `wg show TASK`, then explicitly run `wg retry TASK --reason <WHY>` if replay is safe
```

This is a provider-availability result, not a quality score. The projection must
not use `score`, `semantic failure`, `rejected`, or `accepted`. For an ambiguous
outcome, the single action is to inspect/reconcile the named operation; it must
not recommend blind retry.

A compact state in existing list/TUI rows is sufficient if those surfaces
already render task detail. New dashboards, controls, and rich TUI workflows are
out of V1. The implementation still requires a scripted terminal human-flow
check of the actual `wg show`/status output.

## 10. Minimal configuration

V1 exposes only the narrow source policy:

```toml
[coordinator.source_provider_retry]
enabled = false
max_automatic_retries = 3
recovery_window_seconds = 900
base_seconds = 30
delay_cap_seconds = 300
```

`jitter_divisor = 4` is a V1 protocol constant, not another operator dial.
Validation is intentionally restrictive:

```text
0 <= max_automatic_retries <= 3
1 <= recovery_window_seconds <= 3_600
1 <= base_seconds <= delay_cap_seconds <= recovery_window_seconds
```

`max_automatic_retries = 0` is equivalent to no automatic retry even when the
section is enabled. Existing episodes retain their policy snapshot; config edits
do not rewrite deadlines or counters. A lower time setting is useful for
credential-free smoke tests, while the production defaults remain 3 retries and
15 minutes.

V1 deliberately rejects hour/day windows and more than three automatic retries.
The earlier 24-hour falloff proposal is superseded by this bounded attended
recovery contract.

## 11. Implementation seams and deferred work

### 11.1 Required V1 seams

1. **Classifier/metadata:** add direct evidence provenance, absolute
   Retry-After, operation/failure IDs, execution-outcome safety, and exact
   route/plan IDs to source failure evidence.
2. **`wg fail` transaction:** after exact `AttemptFailed`, create/update the
   embedded episode only for eligible source evidence; bypass cycle restart for
   that condition.
3. **Direct coordinator:** include due failed-source episodes in existing
   capacity/order selection, apply one atomic authorization +
   `GenerationCreated`, and feed its exact binding to the normal spawn path.
4. **Spawn/attempt metadata:** persist episode/authorization/route/plan IDs and
   recover Authorized/Running across restart.
5. **Completion boundary:** refuse enrollment when candidate/finalization
   authority already exists, and preserve late-result publication fences.
6. **Show/status:** render Backoff, Running, Recovered, Paused, and
   NeedsAttention with attempts/window/evidence and one next action.

No required V1 seam needs `src/service/planner.rs`, a new service-owned state
file, an evaluation record transition, or an Agency event.

### 11.2 Explicitly deferred

- automatic evaluator, FLIP, reviewer, or completion-review provider retries;
- changing the existing bounded evaluator's own policy;
- evaluation or Agency retry recommendations, learning, reward, or projections;
- route-wide health aggregation, circuit breakers, probe leases, recovery
  staggering, or fleet storm control beyond existing capacity limits;
- cross-route/model/provider fallback;
- hour/day unattended retry windows or unbounded transient retries;
- generalized durable scheduler/effect architecture;
- synthetic controller/retry graph tasks;
- rich TUI configuration, dashboards, interactive controls, or notification
  routing; and
- automatic remediation for auth, configuration, credit, ambiguous execution,
  semantic rejection, or completion-authority failure.

Request-local HTTP retry may remain inside one provider operation. It emits one
terminal source failure only after its local policy is exhausted and does not
consume multiple V1 lifecycle retries.

## 12. Acceptance cases

### 12.1 Unit tests

1. Table-test every section 4 row, including 429, 500/502/503/504/529,
   unavailable/overloaded, pre-write transport failure, reconciled reset,
   ambiguous reset, 401/402/403, input 4xx, hard timeout, semantic rejection,
   reviewer failure, completion blocker, pause/cancel, text-only, and unknown.
2. Prove direct structured evidence wins over prose and no model/evaluator
   classifier is called.
3. Test the exact formula for retries 1-3, deterministic BLAKE3 byte order,
   jitter bounds, saturation, configured cap, and candidate time at/beyond the
   15-minute boundary.
4. Normalize Retry-After delta/date/malformed values. Prove it is a lower bound,
   duplicate values merge by maximum, and a value beyond the remaining window
   yields NeedsAttention with zero early call.
5. Fold wrapper/fail/telemetry observations with the same `operation_id`; prove
   one `failure_id`, one episode, no budget reset, and no new jitter. A new
   physical operation gets a new failure ID.
6. Test every reset/non-reset event in section 8, especially output bytes,
   restart, generation creation, duplicate failure, and unpause.
7. Prove route/plan mismatch, candidate selection, a completion blocker, and a
   stale lifecycle fence prevent authorization.
8. Prove count and elapsed budgets are independent, and exhaustion never writes
   a score or semantic rejection.

### 12.2 Fake-clock integration tests

These exercise graph persistence, lifecycle, and `coordinator_tick`; they are
integration tests rather than unit mocks unless an existing harness seam
explicitly permits otherwise.

1. With policy disabled, fail a source with direct 429 and prove it remains
   Failed with no automatic episode/generation.
2. Enable policy; at one second before `next_retry_at`, tick with capacity and
   prove no mutation. At eligibility, prove exactly one lifecycle
   `GenerationCreated`, one consumed retry, and one exact-binding spawn.
3. Fail retries 1-3 without authoritative success. Prove no fourth generation,
   NeedsAttention, saved WIP/evidence, and the exact manual next action.
4. Advance beyond 15 minutes with attempts remaining. Prove window exhaustion,
   not a late retry.
5. Supply Retry-After at the deadline and just after it. The first may start at
   the boundary if the coordinator is on time; the second becomes attention.
6. Restart during Backoff, after authorization/before spawn, and after launch
   permit. Prove the same timestamps/counters and at most one owner/generation.
7. Pause across the deadline, unpause, cancel, and disable/re-enable policy.
   Prove no reset or hidden call and manual authority wins.
8. Persist incidental stream output/heartbeats between failures. Prove neither
   budget resets; then persist an exact successful terminal receipt and prove
   the episode closes Recovered.
9. Change handler, model, reasoning, endpoint fingerprint, route ID, plan ID,
   generation, attempt, fence, goal/requirements, candidate, or completion
   blocker before due. Every case is inert/NeedsAttention and never falls back.
10. Deliver a late completion from the superseded attempt and prove existing
    publication/lifecycle fences reject it.

### 12.3 Credential-free real-entry-point smoke

Add a grow-only smoke scenario owned by the implementation/E2E task. It must use
an explicit project-local candidate `wg` binary, the real service/coordinator
entry point, project-local scratch, and a scripted local fake provider—never WG
credentials, a global install, or a root-checkout daemon.

The scenario configures short test-safe values (for example 1-second base,
2-second cap, 30-second window) and proves:

1. disabled policy performs zero automatic retries;
2. direct source 429 plus Retry-After is visible, does not run early, survives a
   service process restart, and runs once on the identical handler/model/route;
3. three failed authorized retries yield NeedsAttention and the fake provider
   receives no fourth request;
4. Retry-After beyond the remaining window yields attention immediately;
5. a reconciled pre-write/reset case retries, while an ambiguous post-write
   reset never does;
6. partial work remains in the retained worktree and the fresh attempt can
   continue it;
7. auth/config/credit, semantic rejection, completion blocker, user pause, and
   evaluator/reviewer failure trigger no source retry;
8. exact route drift and a late old-owner completion are refused by existing
   bindings/fences;
9. graph inspection finds no planner/controller/retry/evaluator task and no new
   scheduler state file; and
10. a scripted human terminal flow runs real `wg show` and service status,
    observes Backoff and NeedsAttention with attempts/window/evidence, follows
    the single explicit recovery instruction, and sees a new operator episode
    rather than a silently reset automatic budget.

The smoke must follow the repository's existing process cleanup contract and
leave no daemon, worker, fake-provider, or Pi process group behind.

## 13. Summary invariant

V1 is one bounded exception to direct fail-stop: exact, direct, safely replayable
source-provider failure may authorize at most three fresh source generations on
the same route within fifteen minutes. The graph and lifecycle remain the
persistence and mutation authority. Everything semantic, ambiguous,
credential/configuration/credit-related, completion-related, paused/cancelled,
evaluator/reviewer-related, fleet-wide, or long-running stays outside automatic
recovery and requires its existing explicit authority.
