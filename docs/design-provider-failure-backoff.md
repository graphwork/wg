# Provider-failure retry and falloff contract

**Status:** accepted design for `provider-backoff-planner`,
`provider-backoff-evaluation`, and `provider-backoff-ux`

## 1. Decision

WorksGood will retry only failures proven to be provider or transport
infrastructure failures. One durable `PlannerStore` retry series owns the next
eligible time and emits one idempotent, exact-route effect. A source failure may
create a fresh lifecycle generation for the same goal; an evaluator or reviewer
failure retries the same immutable candidate record. Neither path changes route,
invents a score, or asks another model to classify the failure.

Transient retry count is unbounded. Its delay is bounded by configurable
exponential falloff with a **24 hour default computed-delay cap**. Reaching that
cap keeps the work live; it never converts infrastructure failure into semantic
`Failed`.

This is deliberately a scheduler contract, not an LLM controller task and not a
revival of `ConvergenceState`.

## 2. Repository reality and the activation boundary

There is an important current-source contradiction which the implementation
must resolve explicitly rather than hiding behind documentation:

- `PlannerStore` calls its decision trace the scheduling authority, its effect
  journal the execution/acknowledgement authority, and its state file a
  rebuildable cache (`src/service/planner.rs:1988-1997`). It already allocates
  monotonic sequence/logical time, persists the trace before returning effects,
  reconstructs issued effects after a crash, and exposes the earliest durable
  deadline (`src/service/planner.rs:2148-2255`, `:2386-2404`).
- The current service says the opposite: startup does not open `PlannerStore`
  and enables direct fail-stop dispatch (`src/commands/service/mod.rs:3034-3040`);
  `coordinator_tick` treats historical planner effects as evidence only
  (`src/commands/service/coordinator.rs:2438-2447`).

The provider-backoff implementation is therefore a narrow, visible cutover:
`PlannerStore` becomes the **only authority for automatic provider-failure retry
eligibility, route probing, and retry effects**. It need not become a semantic
controller or replace ordinary first-attempt readiness. Direct dispatch may
continue for an ordinary open task, but it cannot see a provider-failed source
as open until the due planner effect has passed the lifecycle fence. Evaluation
lanes cannot claim a retry-backoff record until the due planner effect has
rearmed that exact record.

If this cutover is not wired, this design is not implemented. Merely adding a
second timestamp beside direct polling would create two schedulers.

## 3. Existing seams to preserve

### 3.1 Typed failure and telemetry seam

`FailureSignal` already separates a machine reason from human failure prose and
carries HTTP status, provider type/code, `retry_after_secs`, executor, exact
route, and detection time (`src/graph.rs:233-311`). The shared telemetry
normalizer maps structured HTTP/provider evidence before falling back to text
and assigns lower confidence to text/unknown inference
(`src/telemetry/mod.rs:240-299`). Wrapper/raw-stream classification and native
HTTP execution share that normalizer (`src/commands/spawn/raw_stream_classifier.rs:1-109`).

The task failure path persists the signal on the exact task attempt and appends
rolling telemetry; duplicate append sites currently converge by
`(task, attempt, executor, bucket)` (`src/commands/fail.rs:93-140`, `:185-269`;
`src/telemetry/mod.rs:310-337`, `:401-433`). Rolling telemetry and its
`cooled_until_ms` aggregate are useful projections, but are not restart-stable
scheduling authority (`src/telemetry/mod.rs:447-500`).

The implementation must add a bounded provenance discriminator to normalized
failure evidence, for example:

```text
FailureEvidenceKind = HttpResponse | ProviderEnvelope | TransportError |
                      ProcessOutcome | LegacyText | Unknown
```

Automatic retry requires direct evidence: a structured HTTP/provider envelope,
a typed transport timeout/reset, or a typed process outcome whose category is
provider infrastructure. `LegacyText` and `Unknown` can remain visible but
cannot alone authorize automatic work. A model evaluator must never be invoked
to turn untyped prose into provider evidence.

### 3.2 Exact route and route health seam

`HealthRouteKey` already derives a non-secret handler + provider + endpoint
fingerprint from the canonical `SpawnPlan`; credential values are excluded
(`src/service/provider_health.rs:14-82`). `PlannerStore` already has a safe
`DispatchEffectBinding { route_id, plan_id, ... }`, explicitly making route or
model fallback unrepresentable (`src/service/planner.rs:209-223`). It also has a
route projection and one probe lease (`src/service/planner.rs:267-353`) plus
same-route outage/probe/recovery logic (`src/service/planner.rs:1178-1453`).

The retry contract reuses those identities. The raw handler-first route remains
in the domain projection shown to the user; the planner trace carries only the
safe `route_id` and digest of the exact resolved plan.

### 3.3 Evaluation and completion-review seams

An `EvaluationRecord` is already bound to source task, generation, source
attempt/fence, finalization round, candidate and manifest digests, validation
result, policy, and an exact route snapshot (`src/evaluation/mod.rs:123-218`).
An evaluation attempt separately records its exact route, usage, response
digest, and typed infrastructure failure (`src/evaluation/mod.rs:82-121`).

The bounded lane currently computes its own 15-second exponential timer and a
three-process-attempt limit in `is_claimable`
(`src/evaluation/bounded.rs:39-40`, `:568-605`). Its finalizer correctly keeps
infrastructure failure separate from semantic rejection and leaves source
lifecycle unchanged (`src/evaluation/bounded.rs:984-1050`), while semantic
verdict consumption is candidate/route CAS-bound and idempotent
(`src/evaluation/bounded.rs:775-978`). The local retry timer must be retired for
typed provider failures; the exact record/CAS and verdict authority remain.

Completion review likewise has an exact manifest/requirements/source binding
and represents `SemanticRejection`, `ReviewerUnavailable`, and
`IncompleteEvidence` separately (`src/completion_review.rs:90-145`). A reviewer
unavailable result becomes an immutable unavailable receipt, not a rejection
(`src/completion_review.rs:991-1081`). The reviewer adapter must attach direct
normalized provider evidence when it has it; its current code/message-only
`ReviewerUnavailable` is insufficient to authorize automatic retry.

## 4. Non-negotiable invariants

1. **Direct evidence first.** Structured provider/transport evidence wins.
   No model call exists solely to rediscover a 429, timeout, reset, or 5xx.
2. **Same route.** Every automatic retry uses the exact handler, provider,
   model, endpoint fingerprint, reasoning, and route generation captured by the
   failed operation. There is no implicit profile, tier, model, endpoint, or
   executor fallback.
3. **Same work identity.** Source retry retains the same goal and completion
   contract. Evaluation/review retry retains the exact candidate, manifest,
   policy, reviewer kind, and evaluation/review binding.
4. **One scheduler.** `PlannerStore` owns `next_eligible_at`, retry ordinal,
   route breaker, probe lease, and effect issuance.
5. **Domain authority remains.** A planner effect requests an existing
   lifecycle/evaluation/review transition. It cannot write source status,
   create a quality score, accept/reject a candidate, or edit source.
6. **One physical operation at a time.** An active exact attempt/claim or a
   spawned route-probe lease prevents a second operation for the same target.
7. **Ambiguous outcome is fail-closed.** Every physical provider operation gets
   a stable `operation_id` before network I/O. A timeout/reset may authorize a
   fresh lifecycle attempt only when the adapter can prove the prior operation
   is replay-safe: the provider honors that idempotency identity, reconciliation
   proves no accepted result, or no externally committed tool effect can be
   duplicated. Otherwise it becomes `ambiguous-provider-outcome` and waits for
   reconciliation/operator evidence.
8. **Elapsed time is not semantic evidence.** It may make a transient retry due;
   it can never re-evaluate a consumed rejection or turn transient failure into
   semantic failure.
9. **Crash safety before execution.** An effect is in both planner trace and
   effect journal before an adapter sees it. Adapter execution and
   acknowledgement use the same stable effect ID.

## 5. Failure-class decision table

“Automatic” below means planner-authorized after falloff, not an in-process HTTP
client retry. Request-local retries inside one executor call may remain; after
those are exhausted they emit one typed attempt failure into this contract.

| Evidence and locus | Canonical class | Automatic action | Source/candidate consequence | Wake/reset condition |
|---|---|---|---|---|
| Source worker: direct HTTP 429, valid `Retry-After`, transient 5xx, typed provider overloaded/unavailable, transport timeout/reset/DNS/connect failure, with replay safety proved | `transient-provider` | Schedule a fresh lifecycle-authorized generation of the same goal on the exact failed route; coordinate through the route breaker/probe lease | Preserve source/WIP evidence; do not spend semantic/cycle retry budget | Due deadline and route lease; reset only on authoritative provider/source progress |
| Evaluator or reviewer: same direct infrastructure evidence | `transient-provider` | Rearm and run the same evaluation record or review binding on the exact route | Never rerun unchanged source; no score/verdict is created for the failed call | Due deadline and route lease; reset on a well-formed result or a new candidate |
| Evaluator malformed output or insufficient/missing evidence without direct provider evidence | `evaluation-evidence` or `unknown` | No provider automatic retry. Existing evidence-repair/manual policy may act, but not this scheduler | Source remains unchanged and awaiting the correct evidence | New evidence/candidate or explicit operator retry |
| Semantic evaluator/reviewer reject with a durable verdict/receipt | `semantic-rejection` | None | Keep the rejected immutable candidate; require source repair, waiver, or a genuinely new candidate | New candidate/manifest or audited operator action only; time never requeues it |
| Source validation/deliverable/source-quality failure, task input 4xx, context limit, agent hard timeout, clean no-op | `source-quality` | None under provider policy | Follow explicit source repair/retry policy; do not call it provider recovery | New source/operator evidence |
| 401/403, invalid/missing key, missing handler/adapter, invalid endpoint/model, route drift, executor config | `auth-config` | No timed credential-bearing retry. Persist operator/config wait | Keep same goal or record and exact route identity | A new credential/config/route-validation event; then start at base delay or probe once |
| 402, insufficient credits, exhausted account/project budget | `credit-exhausted` | No aggressive timed retry and no fallback | Keep same goal or record; show credit action needed | A credit/budget event or explicit operator retry; then one same-route probe |
| Timeout/reset after possible provider acceptance or external tool commit, without idempotency/outcome proof | `ambiguous-provider-outcome` | No automatic retry | Preserve exact session/WIP and fence late results; reconcile by `operation_id` or require operator action | Durable outcome/replay-safety evidence |
| Text-only inference, generic nonzero exit, contradictory evidence, or `FailureReason::Unknown` | `unknown` | No automatic retry | Preserve evidence and create a typed reconciliation/operator recommendation, no score | Direct evidence arrives or explicit operator action |

A provider’s `Retry-After` on an authentication/credit error does not make that
hard class transient. Conversely, a transport timeout is transient only when it
is the provider request/stream transport; a whole-agent hard timeout remains a
source/task failure unless direct nested provider evidence proves otherwise.

## 6. Durable retry model

Extend planner state with one bounded retry projection per target series:

```text
RetrySeries {
  retry_id,                 // stable series identity
  target: Source(SourceRetryKey) | Evaluation(EvaluationKey) | Review(ReviewKey),
  operation_id,             // minted and persisted before physical I/O
  route_id,
  plan_id,
  progress_id,
  last_failure_id,
  replay_safety_receipt_id,
  class,
  failures_without_progress,
  base_seconds,
  cap_seconds,
  jitter_divisor,
  retry_after_not_before,   // canonical absolute Unix-second lower bound
  next_eligible_at,
  pending_effect_id,
  disposition: Backoff | AwaitRouteProbe | AwaitOperatorEvent | Due,
}
```

`SourceRetryKey` persists graph ID, task ID, goal digest, completion contract,
failed generation, failed attempt ID, failed attempt fence, and the lifecycle
revision which accepted the failure. `EvaluationKey` includes `evaluation_id`,
the complete `SourceCandidateRef`, policy digest, route digest, and last failed
evaluation attempt ID. `ReviewKey` includes task/generation/attempt/fence,
candidate sequence, manifest and requirements digests, reviewer kind, exact
route digest, and unavailable receipt ID. The full target key,
`operation_id`, `failure_id`, route/plan IDs, progress ID, and replay-safety
receipt ID are inputs to the retry effect ID. A later failure or generation
therefore cannot match an old effect even if it uses the same task and route.
Mutable prose, API keys, paths, prompts, and candidate bytes are not planner
fields.

The dispatch/evaluation/review adapter mints `operation_id` transactionally
before the first physical request and puts it in spawn/claim metadata. Provider
request/event IDs are aliases attached afterward, never alternate identities.
`failure_id` depends on the canonical operation identity, not on whichever
observation site saw the error:

```text
failure_id = blake3("wg-provider-failure-v1" || target exact tuple ||
                    operation_id || route_id || plan_id)
```

A physical operation has one terminal failure observation. Wrapper,
task-failure, telemetry, and provider-envelope reports for that operation merge
under the same `failure_id`. The reducer selects evidence by the fixed
precedence `ProviderEnvelope > HttpResponse > TransportError > ProcessOutcome >
LegacyText > Unknown`, takes the maximum `retry_after_not_before`, and requires
all direct reports to agree on the hard/transient class. A conflicting direct
class becomes `ambiguous-provider-outcome`; it never creates a second failure.
Detection time, optional provider event ID, status/type/code spelling, and human
prose cannot change identity. A new physical retry receives a new
`operation_id`, so it advances the exponent exactly once.

A retry series crosses automatic source generations: `GenerationCreated` alone
does not reset it. It ends or resets only by the progress rules in §8.

### 6.1 Replay-safety receipt

Automatic source retry requires an immutable, content-addressed
`ReplaySafetyReceipt`:

```text
ReplaySafetyReceipt {
  schema: 1,
  receipt_id,                 // digest of canonical fields below, excluding this field
  operation_id,
  target: SourceRetryKey,
  route_id,
  plan_id,
  failed_execution_id,        // launch receipt or planner effect-execution ID
  proof: DefinitiveProviderRejection { status, provider_request_id } |
         PreWriteTransportFailure { connected, bytes_written: 0 } |
         ProviderIdempotencyBound { idempotency_key, provider_request_id } |
         ReconciledNoResult { provider_query_receipt_id },
  external_effect_journal_head,
  issuer: { adapter_kind, adapter_binary_digest, run_id },
  evidence_refs,
  issued_at,
}
```

The exact execution adapter which owns `operation_id` is the only issuer. It
writes the canonical receipt to the immutable completion/evidence object store
and links its digest to the operation/effect execution journal before reporting
the failure to `PlannerStore`. Planner input carries only the receipt digest.
Before effect issuance and again before lifecycle mutation, the verifier reloads
that object and checks: content digest; schema; registered adapter/run identity;
exact operation, source, route, plan, and failed launch/effect-execution
bindings; and the external-effect journal head. `DefinitiveProviderRejection` accepts only an
actual non-success provider response such as 429/5xx, never a missing response.
`PreWriteTransportFailure` requires the transport journal to prove zero request
bytes were written. `ProviderIdempotencyBound` requires the route capability
snapshot and provider response metadata to prove the key was accepted;
`ReconciledNoResult` requires a provider query receipt bound to the provider
request ID. A timeout after any request bytes, an unsupported idempotency key,
missing journal data, unknown issuer, digest mismatch, or a later external tool
commit fails closed. “No visible graph result” is not proof.

Evaluation/review retries also carry an operation receipt when their outcome is
ambiguous; a definitive 429/5xx receipt is sufficient. The receipt authorizes
only replay safety. It does not classify quality, grant lifecycle authority, or
prove route health.

## 7. Exact falloff formula

For failure ordinal `n = failures_without_progress` before recording the new
unique failure (first failure uses `n = 0`), with normalized
`1 <= base_seconds <= cap_seconds`, and `jitter_divisor >= 1`:

```text
raw(n)       = min(cap_seconds,
                   saturating_mul(base_seconds, 2^min(n, 63)))
window(n)    = floor(raw(n) / jitter_divisor)
jitter(n)    = H("wg-provider-retry-jitter-v1" || retry_id || progress_id || n)
               mod (window(n) + 1)
computed(n)  = min(cap_seconds, raw(n) + jitter(n))
eligible_at  = max(observed_at + computed(n),
                   retry_after_not_before.unwrap_or(0))
```

Before planner observation, the adapter canonicalizes `Retry-After` once. A
finite nonnegative delta becomes
`retry_after_not_before = observed_at + ceil(delta_seconds)`; an HTTP date
becomes its ceiling Unix-second timestamp. Absence or malformed/NaN/infinite/
negative input becomes `None` plus a diagnostic. Duplicate reports merge with
`max(existing, incoming)`, so the lower bound can never move earlier. After
computing this unique failure, persist
`failures_without_progress = n + 1`. All arithmetic is
saturating. `H` is the first eight BLAKE3 digest bytes interpreted as a
little-endian unsigned 64-bit integer. The hash inputs and policy snapshot are
persisted, so the result is deterministic across restart and machines. The
positive jitter is in
`[0, floor(raw/jitter_divisor)]`; `computed` never exceeds the configured cap.
At the cap, positive jitter may saturate to the cap, which is acceptable because
the route breaker still admits only one probe and recovery release is separately
staggered per target.

`Retry-After` is a protocol lower bound. It may extend `eligible_at` beyond the
local computed-delay cap; silently shortening it would violate the provider’s
instruction. Thus “24 hour cap” means the maximum delay **generated by WG’s
falloff**, not permission to ignore a longer provider lower bound.

The raw header form is diagnostic evidence only; planner state and replay use
only the canonical absolute `retry_after_not_before`.

## 8. Progress and reset rules

There are two related scopes.

### 8.1 Route scope

A route outage count resets only on a durable success receipt from a
credential-bearing operation on the exact `route_id`/`plan_id`: the leased real
source/evaluation/review operation or an explicit route probe. Config reload,
time passage, a new task, an attempted spawn, or an unverified health guess does
not mark the route healthy.

### 8.2 Target scope

The target series resets when its stage-aware `progress_id` advances:

- source: a new exact candidate/manifest plus validation result, a completed
  source result, or a durable non-provider semantic disposition;
- evaluation: a well-formed semantic verdict or a genuinely new candidate,
  policy, or explicit route generation;
- review: a well-formed bound review receipt or a new candidate/manifest/
  requirements binding;
- hard wait: an explicit credential, config, or credit event starts a new
  eligibility epoch, without silently changing route.

The following never reset falloff: `GenerationCreated`, reservation, spawn,
claim, heartbeat, PID liveness, output bytes, tokens, logs, duplicate telemetry,
an identical failure, coordinator restart, or status rendering. A successful
provider response followed by semantic validation failure resets provider
health, then follows semantic/source policy; it does not continue a provider
retry loop.

Policy changes do not rewrite an already persisted deadline, exponent, or
jitter. New defaults apply to a new series; an explicit operator “recompute
retry policy” event may start a new policy epoch and is audited.

## 9. One route probe lease

Every target joins one durable route projection keyed by `HealthRouteKey`:

```text
RouteRetryState {
  route_id,
  route_epoch,
  state: Healthy | Unavailable | Probing | AwaitOperatorEvent,
  consecutive_probe_failures,
  last_outage_failure_id,
  retry_after_not_before,
  next_probe_at,
  route_probe_base_seconds,
  route_probe_cap_seconds,
  jitter_divisor,
  probe_lease,
  recovered_at,
}
```

The first unique transient failure opens an outage epoch. Concurrent failures
from operations admitted before the breaker opened join that epoch, update the
maximum `retry_after_not_before`, and **do not** increment the route exponent.
Only a failed operation holding `probe_lease` increments
`consecutive_probe_failures`; duplicate failure IDs are inert. For route ordinal
`m = consecutive_probe_failures` before the failed probe (the initial outage
uses `m = 0`):

```text
route_raw(m) = min(route_probe_cap_seconds,
                   saturating_mul(route_probe_base_seconds, 2^min(m, 63)))
route_window = floor(route_raw(m) / jitter_divisor)
route_jitter = H("wg-route-probe-v1" || route_id || route_epoch || m)
               mod (route_window + 1)
route_delay  = min(route_probe_cap_seconds, route_raw(m) + route_jitter)
next_probe_at = max(failure_observed_at + route_delay,
                    retry_after_not_before.unwrap_or(0))
```

After computing a failed leased probe, persist
`consecutive_probe_failures = m + 1`. The same saturation, little-endian BLAKE3,
absolute Retry-After lower-bound, and invalid-value rules from §7 apply. Route
state, ordinal, policy snapshot, epoch, lower bound, deadline, and lease are in
`PlannerStore` trace/state, so restart neither redraws jitter nor makes a probe
early. Success persists `Healthy`, increments `route_epoch`, clears the ordinal,
lower bound, deadline, and lease, and records `recovered_at` before releasing
waiters. Auth/config/credit changes instead enter the event-gated state below.

While a route is unavailable:

1. all target retry records retain their own stage and exact route;
2. the earliest due target may acquire the route’s single `probe_lease`;
3. that lease authorizes one real, exact waiting operation as the probe whenever
   possible—source retry, evaluation retry, or review retry—rather than creating
   a synthetic graph task or LLM controller;
4. other targets remain `AwaitRouteProbe` even if their local deadlines pass;
5. success closes the breaker and releases waiting targets with deterministic
   route-epoch/task staggering; failure advances the route outage falloff once.

The recovery staggering formula is exact. Persist `recovered_at` and incremented
`route_epoch`; for each waiting retry series:

```text
stagger_window = route_probe_base_seconds
stagger = H("wg-route-recovery-v1" || route_id || route_epoch || retry_id)
          mod (stagger_window + 1)
release_at = max(next_eligible_at, recovered_at + stagger)
order = (release_at ascending, retry_id lexicographic)
```

`H` uses the same little-endian BLAKE3 rule as §7. A zero window releases at
`recovered_at`. The persisted route epoch, recovery time, retry ID, and target
deadline make the release stable across restart; the lexicographic tie-breaker
makes collisions deterministic.

A lease binds effect ID, target key, route/plan IDs, lease epoch, and expiry. A
lease which has not started may expire and be reacquired. Once its physical
operation is recorded as started/spawned, it has no time-only expiry: the exact
owner/claim must finish or be proven dead and fenced before another probe can
run. This matches the existing planner distinction between an unspawned lease
with `expires_at` and a spawned lease with no expiry
(`src/service/planner.rs:1318-1354`, `:1651-1674`).

Authentication, configuration, and credit exhaustion put the route in
`AwaitOperatorEvent`, not ordinary timed probing. A slow 24-hour safety wake may
refresh non-credential diagnostics, but it cannot invoke the provider or rearm
work. A qualifying operator/config/credit event can authorize one probe at base
delay. It cannot select a fallback.

## 10. Automatic transition and idempotency fences

### 10.1 Source worker failure

The failed attempt is first terminalized normally through
`AttemptFailed` with the exact generation/attempt/fence expectation. Only then
may the planner schedule retry. At due time its effect adapter must atomically
re-read and verify:

- every persisted `SourceRetryKey` field: graph/task ID, goal digest, completion
  contract, failed generation, failed attempt ID/fence, and accepted failure
  revision;
- current attempt is terminal `Failed` for the same `failure_id` and no owner is
  live;
- task failure projection still contains the same direct failure and exact
  route/plan binding;
- no newer candidate, verdict, operator edit, abandonment, or generation exists;
- the planner effect ID and route probe lease are current; and
- `replay_safety_receipt_id` resolves and verifies under §6.1 for the exact
  operation/source/route/plan/effect-execution tuple. A possibly accepted
  provider result or externally committed tool effect without such a receipt
  changes the series to `ambiguous-provider-outcome` instead of creating a
  generation.

The adapter then requests existing lifecycle
`TransitionKind::GenerationCreated` as `ActorKind::Reconciler`, with full
`FenceExpectation`, evidence refs containing `failure_id` and effect ID, and:

```text
idempotency_key = "provider-source-retry:" + failure_id
reason_code     = "provider_failure_retry_due"
```

`GenerationCreated` is already the lifecycle-authorized transition which
increments generation, clears the current attempt, and returns a terminal
source to `Open` (`src/lifecycle.rs:897-918`). The planner does not write status.
A normal dispatcher reservation then creates a fresh attempt/fence. That
attempt must recompute the canonical plan and match the persisted `plan_id`; a
mismatch becomes `auth-config`/route-drift wait, never fallback.

A crash before lifecycle commit leaves the effect replayable. A crash after
commit finds the lifecycle idempotency key and returns the same event; it cannot
increment generation twice. The planner acknowledges success only after that
event is durable. Existing `retry_count`, `max_retries`, rescue count, and cycle
`restart_on_failure` are not consumed or triggered by this infrastructure
retry.

If failure is observed while an exact owner is still live, use the existing
fenced reopen/owner-release protocol before generation creation; never create a
concurrent generation from time alone.

### 10.2 Evaluator failure

The planner effect does **not** call `GenerationCreated` and does not mutate
source status. Its adapter CAS-checks the same `evaluation_id`, complete
`SourceCandidateRef`, policy/route digests, last failed attempt ID, failure ID,
`RetryBackoff` state, and absence of a consumed verdict. It then re-arms that
same record for the dedicated lane:

```text
idempotency_key = "provider-evaluation-retry:" + failure_id
transition      = RetryBackoff -> Queued (same record)
```

The lane claims it once and appends a new `EvaluationAttempt`; it may not mint a
new `EvaluationRecord`, change route, or rerender from changed source. A stale
candidate, changed route digest, consumed verdict, or different last attempt
rejects the effect as stale. A stale rejection is terminal acknowledgement, not
a reason to rerun source.

### 10.3 Reviewer failure

The planner re-invokes the same reviewer kind against the exact immutable
manifest/requirements/source binding and route. The unavailable receipt remains
immutable audit evidence; a new attempt produces a new receipt linked to the
same retry series. The effect is fenced by candidate sequence, generation,
attempt/fence, manifest and requirements digests, reviewer kind, failed receipt
ID, and route digest:

```text
idempotency_key = "provider-review-retry:" + failure_id
```

If FLIP was unavailable, eval remains uncalled because the valve ordering still
applies. If eval was unavailable after a passing FLIP, the existing passing FLIP
receipt may be reused only when every binding digest is unchanged. Neither case
restarts source.

### 10.4 Semantic result

A real verdict continues through the existing evaluation/review acceptance
owner. Provider-retry effects have no `AcceptanceSatisfied` or
`AcceptanceRejected` capability. A consumed reject has no deadline and is never
rearmed because wall time advanced. Only a new candidate or explicit audited
operator retry can create new semantic work.

## 11. User-visible retry recommendation

`wg show --json`, `wg service status --json`, and evaluation/review projections
should expose a joined planner read model, not mutate an immutable verdict or
receipt:

```text
RetryRecommendation {
  schema: 1,
  target: source | evaluation | review,
  disposition: automatic-same-route | await-operator-event | none,
  failure_class: transient-provider | auth-config | credit-exhausted |
                 ambiguous-provider-outcome | semantic-rejection |
                 source-quality | unknown,
  reason_code,
  evidence_id,                 // failure_id or immutable receipt id
  exact_route,                 // redacted handler-first route, no credential
  route_id,
  same_route: true,
  failures_without_progress,
  computed_delay_seconds,
  retry_after_lower_bound_seconds,
  next_eligible_at,
  pending_effect_id,
  probe_lease_state,
  operator_event_required,
}
```

Nullable timing/effect fields are omitted for `await-operator-event` and
`none`. Semantic rejection may be rendered as `disposition=none,
reason_code=semantic_rejection_requires_changed_candidate`; it has no
`next_eligible_at`.

A recommendation is not a verdict and contains no `score`, `outcome`,
`dimensions`, or synthetic finding. Human output should be direct, for example:

```text
Retry: automatic evaluator retry on the same route
Reason: provider rate limit (direct HTTP 429)
Eligible: 2026-08-17T03:04:05Z (failure 4; Retry-After lower bound honored)
Candidate: unchanged eval-… / wgcid:…
```

Hard classes instead show the exact event needed: authenticate/configure the
same route, add credits/raise budget, or request operator classification.

## 12. Configuration and migration

Use the existing durable convergence policy surface rather than a second retry
configuration namespace:

```toml
[convergence]
base_seconds = 30
cap_seconds = 86400
route_probe_base_seconds = 30
route_probe_cap_seconds = 86400
action_lease_seconds = 300
jitter_divisor = 4
```

The fields already exist in `ConvergenceConfig`
(`src/config.rs:4470-4508`). Current defaults are 5 seconds and 6 hours for the
general falloff, and 30 seconds/1 hour for route probes
(`src/config.rs:4926-4955`). The accepted provider-retry defaults change both
caps to **86,400 seconds (24 hours)** and use 30 seconds as the first new-series
delay. There is no compatibility evidence requiring a staged 6h→24h migration.

Durations are integer seconds, permitting second/minute initial delays and
hour/day-order caps without a new duration parser. Validation rejects zero,
`base > cap`, or values which cannot be represented safely. Existing persisted
series retain their snapshotted policy and deadline byte-for-byte; new defaults
apply only to a new series. A one-time import may copy legacy state into planner
migration evidence, but may not execute or recompute it.

The cap limits delay, not retries. There is no `max_attempts` for direct transient
provider failures. Existing evaluation `MAX_PROCESS_ATTEMPTS` may continue for
non-provider malformed/evidence policy if desired, but it cannot terminate or
suppress this typed transient-provider series.

## 13. Duplicate authorities that must remain disabled

The implementation must delete, bypass, or reduce to read-only projection every
competing scheduler for this condition:

- **`ConvergenceState` dispatch/route scheduling.** It remains one-time readable
  migration evidence only; its module already says its dispatch claim reducer
  is unreachable (`src/service/convergence.rs:1-6`). Do not call
  `reconcile_dir`, `admit_goal_action`, or `admit_route_action` for provider
  retry.
- **Ephemeral failed-prerequisite planner.** `converge_failed_prerequisites`
  currently constructs a fresh `PlannerState` and can apply a bounded source
  retry (`src/service/convergence.rs:973-1211`). It must not independently
  retry typed provider failures after this cutover.
- **Evaluation lane timer.** `bounded::is_claimable` must not compute a second
  retry deadline from `completed_at`; it may claim only `Queued` records rearmed
  by a due planner effect.
- **Telemetry cooldown.** `ProviderHealth.cooled_until_ms` is status evidence,
  not dispatch admission or a wake timer.
- **Provider/global pause.** `provider_health.service_paused`, auto-resume,
  threshold pause scheduling, and zero-output global backoff remain retired as
  scheduling authority. Route evidence feeds the planner only.
- **Cycle and generic retry.** `evaluate_cycle_on_failure`, `wg retry`, rescue
  counters, rapid-respawn throttles, and `max_retries` cannot automatically
  reopen a task for a direct provider failure. An operator may still explicitly
  retry through its normal audited path.
- **Direct coordinator polling.** It cannot open/rearm provider-failed work or
  replay an effect that is not due. It executes only the planner effect or
  ordinary first-attempt work.
- **Synthetic agency/controller tasks.** No `.evaluate-*`, `.review-*`, route
  probe, retry, supervisor, or controller graph task is created. Evaluation and
  review remain hidden attempt-bound records/receipts.

Request-local HTTP client backoff and the daemon **process** supervisor are not
duplicate lifecycle schedulers: the former stays within one physical attempt;
the latter restarts `wg service`, not graph/evaluation work.

## 14. Acceptance tests

### 14.1 Unit and property tests

1. Table-test every row in §5, including 429 with and without `Retry-After`,
   500/503/529, typed timeout/reset, ambiguous post-accept reset, 401/403,
   402/credits, invalid config, semantic reject, source-quality reject, and
   unknown/text-only evidence.
2. Prove direct structured evidence wins over contradictory prose and no
   evaluator/model classifier is called; conflicting direct evidence fails
   closed as one ambiguous operation.
3. Table-test the exact formula for `n=0`, growth, saturation/overflow,
   deterministic jitter, delta and HTTP-date normalization, malformed
   `Retry-After`, duplicate max-merge, a lower bound below the computed delay,
   and a lower bound beyond 24 hours.
4. Submit wrapper/task/telemetry observations with different optional provider
   event/status details but the same `operation_id`; prove one canonical
   `failure_id`, one series increment, and one effect. A new operation ID must
   increment exactly once.
5. Prove progress rows in §8 reset the correct scope and heartbeat, token,
   output, spawn, generation creation, and duplicate failure do not.
6. Verify every §6.1 receipt proof variant, content/issuer/journal binding, and
   source key/effect identity; reject post-write timeout, missing or mismatched
   receipts, a stale source fence/revision, and a later external effect.
7. Table-test the §9 route formula, initial outage, failed-probe-only exponent,
   concurrent tail-failure merge, Retry-After floor, cap/overflow, restart, and
   success reset.
8. Prove auth/config/credit and unknown evidence have no timed credential-bearing
   effect; a matching operator event enables one same-route probe.
9. Prove semantic rejection has no deadline and remains inert after arbitrary
   fake-clock advancement.

### 14.2 Fake-clock planner tests

1. Source 429: observe the exact terminal attempt, advance to one second before
   eligibility (no effect), then to eligibility (one source-retry effect).
2. Evaluation 503: keep source/candidate unchanged, advance clock, and assert
   only the exact evaluation record is rearmed.
3. Review timeout: assert the same manifest/requirements/reviewer binding is
   invoked and source is not reopened.
4. Drive failures to the 24-hour computed cap, advance several more ordinals,
   and assert the record stays live in backoff rather than becoming semantic
   `Failed`.
5. Seed N targets on one route; assert one probe lease, no parallel physical
   call, the exact §9 release times/tie-break order across restart, and no
   fallback plan ID.
6. Change candidate, lifecycle fence, route plan, or consumed verdict before a
   due effect and assert `RejectedStale` with no mutation.

### 14.3 Restart and crash-order tests

1. Persist a long deadline, counter, policy, progress/failure IDs, Retry-After
   bound, and probe lease; reopen `PlannerStore` and compare them byte-for-byte.
2. Restart before deadline and prove it is not recomputed from restart time.
   Restart after deadline and prove exactly one effect becomes replayable—no
   catch-up loop.
3. Crash at each boundary: trace write, journal issue, execution-start, lifecycle
   or record CAS, physical outcome, and acknowledgement. Replay must create at
   most one source generation or evaluation/review attempt.
4. Crash after `GenerationCreated` but before planner acknowledgement; replay
   must find the lifecycle idempotency event and not increment generation.
5. Prove an already-started probe lease is not replaced on wall-clock expiry
   until its exact owner is terminal/proven dead.
6. Persist a `SourceRetryKey` and replay-safety receipt, then change generation,
   attempt, fence, revision, goal, contract, journal head, or adapter identity;
   every stale/mismatched replay must be inert.

### 14.4 Credential-free smoke

Add one fake-provider scenario owned by the implementation task. It must drive
the real service event loop and deterministic fake clock without credentials:

1. source route returns 429 + Retry-After; no immediate respawn occurs, restart
   preserves deadline, and exactly one fresh same-route lifecycle attempt runs
   when due. A separate post-accept reset fixture without idempotency/outcome
   proof must remain fail-closed and create no generation;
2. evaluator route then returns 503; unchanged source is not run again and the
   same evaluation record retries when due;
3. a semantic reject is consumed once and remains inert after a day;
4. auth, config, and credit fixtures show operator-event waits with zero timed
   calls;
5. multiple tasks on the failed route yield one probe, no fallback, no storm,
   then recover after one probe success;
6. repeated transient failures reach the 24-hour cap while remaining visible
   and nonsemantic;
7. service/status JSON exposes the §11 recommendation fields with no quality
   score; and
8. graph inspection proves no controller/evaluator/reviewer/probe/retry task was
   created.

## 15. Implementation order

1. Add direct-evidence provenance and a stable failure ID at the shared
   classifier/telemetry boundary.
2. Add retry observations/series/effects to the pure planner and replay tests.
3. Wire the service event loop to `PlannerStore` for this narrow authority and
   include `read_earliest_deadline` in wake calculation.
4. Implement the fenced source lifecycle adapter and same-plan check.
5. Replace evaluation’s local timer with exact-record planner rearm; add the
   equivalent immutable review binding.
6. Join planner retry recommendations into status/show JSON and human output.
7. Remove or assert-unreachable duplicate authorities in §13, then enable the
   credential-free smoke.

At every step, provider retry remains a deterministic consequence of typed
evidence. Evaluation observes quality; it never owns time, source lifecycle, or
provider-error discovery.
