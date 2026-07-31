# Deterministic daemon convergence reconciler

**Status:** decision draft; not approved for implementation or publication

**Operator gate:** this document does **not** supersede any task by itself. No
implementation task may be published until an operator chooses one of the task
migration options in [§12](#12-operator-decision-required). Until then, the
published older controller tasks must remain non-dispatchable.

## 1. Decision in one sentence

The existing `wg service` daemon should own one deterministic, durable scheduler
for convergence. It is not a graph task, an LLM persona, a source-code editor,
or a second lifecycle authority. It wakes the existing domain owners and takes
at most one fenced, idempotent action for one existing goal toward its `land`,
`deliver`, or `report` contract.

This replaces the older four-layer proposal in
`docs/studies/supervisor-hard-agent-design.md`. That proposal adds a slow
"supervisor" reset persona beside the reaper, sweep, evaluation reconciler, and
parallelism controller. Current main now has stronger primitives that the older
study predates:

- lifecycle generation/revision/fence, idempotency keys, and an append-only
  accepted-event ledger (`src/lifecycle.rs:112-176`, `:309-377`, `:1184`);
- task-owned `land`/`deliver`/`report` finish receipts and cleanup-before-`Done`
  (`src/graph.rs:320-365`, `src/finalization/mod.rs:212-303`,
  `src/commands/finalize.rs:458-752`);
- exact route identity for provider health
  (`src/service/provider_health.rs:17-102`);
- event-driven graph wakes plus a background safety timer
  (`src/commands/service/mod.rs:3116-3142`, `:3283-3287`, `:3681-3726`); and
- normalized provider telemetry with persisted cooldown evidence
  (`src/telemetry/mod.rs:448-503`).

The missing piece is therefore scheduling and durable wake policy, not a new
actor that guesses which task status to rewrite.

## 2. Authority boundary

The reconciler owns only:

1. the durable record of what existing goal/stage should be reconsidered next;
2. selection of one due action;
3. a per-action lease, fence expectation, and idempotency key;
4. classification of an action result as progress, semantic redirection,
   transient infrastructure, route outage, or needs-human; and
5. computation of the next durable wake.

It does **not** own the mutations below.

| Domain | Existing authority | Reconciler action |
|---|---|---|
| task status, attempt, generation, fence | lifecycle kernel (`src/lifecycle.rs:1184`) | submit one typed, fenced request through the existing owner |
| dead process/owner | `triage::cleanup_dead_agents` (`src/commands/service/triage.rs:272`) | schedule that pass; consume its authoritative result |
| graph/registry split-save orphan | `sweep::reconcile_orphaned_tasks` (`src/commands/sweep.rs:435`) | invoke as the safety net, never reproduce its mutation |
| explicit task wait | wait matcher (`src/commands/service/coordinator.rs:559-795`) | wake it on graph/message/deadline events |
| evaluation | bounded/deep lanes (`src/evaluation/bounded.rs:417`, `src/evaluation/deep.rs:611`) | run/link one pending exact-candidate record |
| evaluation verdict projection | evaluation/finalization receipt owners (`src/finalization/mod.rs:586-638`) | observe the receipt and select the next stage |
| source integration/promotion | original task owner and finish lease (`src/finalization/mod.rs:640-789`) | wake the same goal owner; never edit or promote for it |
| durable output | finish output publisher (`src/finalization/mod.rs:791-836`) | request the existing idempotent action |
| cleanup and terminal `Done` | `cleanup_finish` (`src/commands/finalize.rs:624-752`) | run cleanup only after a durable disposition receipt |
| route outage | one route-key breaker/probe | schedule one probe; defer every affected goal without rerouting |
| dependency readiness | canonical dependency disposition and unblock scan (`src/commands/service/coordinator.rs:808-907`) | schedule the existing scan |

Existing reaper, sweep, wait, evaluation, finalization, and cleanup code becomes
steps called from the one reconciliation pass. It must not also retain a second
independent retry timer or controller policy for the same condition. Startup
replay (`src/commands/service/mod.rs:2827-2863`) must call the same reconciliation
entry point used after events and deadlines instead of being a separate source
of authority.

## 3. Durable record

Use one atomically replaced daemon read model, for example
`.wg/service/convergence-state.json`. An append-only bounded audit journal may
explain decisions, but it is not scheduling authority. There is no
`.daemon-*`, `.supervisor-*`, probe, merge, cleanup, or controller graph task.

A record exists for every nonterminal goal and is keyed by task id plus lifecycle
generation:

```text
GoalRecord {
  goal: GoalRef {
    task_id,
    generation,
    goal_digest,          // title/description/contract identity, not copied prose
    completion_contract,  // land | deliver | report
  },
  stage,
  blocker,
  next_wake_at,
  backoff: {
    class,
    failures_without_progress,
    base_seconds,
    cap_seconds,
    jitter_seed,
  },
  last_authoritative_progress: ProgressStamp,
  pending_action: Option<ActionLease>,
  needs_human: Option<NeedsHumanRecord>,
}
```

`ActionLease` binds `task_id`, generation, attempt id, lifecycle fence and
revision, stage, action kind, progress stamp, lease epoch, expiry, and a stable
idempotency key. Its action id is a digest of that tuple. A stale lease or stale
fence may record a no-op; it may never apply the action to a newer attempt.

The task graph remains the goal source of truth. `goal_digest` detects an
operator edit and forces re-derivation; it does not freeze or duplicate attacker
text in daemon state. Terminal tasks lose their live record only after their
required completion disposition and cleanup receipt are verified. A compact
tombstone may retain the last action/progress ids for audit and deduplication.

### Stages

The stage is a projection, not a new graph status:

```text
ObserveOwner | AwaitDispatch | AwaitWait | AwaitEvaluation |
AwaitSourceRepair | AwaitSourceFinish | AwaitPromotion |
AwaitCleanup | NeedsHuman
```

`AwaitSourceRepair` covers semantic rejection and real integration/source
conflict. `AwaitSourceFinish` covers accepted candidate evidence whose protected
source-owned action has not completed. Both wake the **original goal task** in
its preserved worktree/session when safely resumable. Neither creates a repair,
merge, evaluator, probe, or cleanup task.

## 4. Authoritative progress

Backoff resets only when the stage-aware `ProgressStamp` advances. A stamp may
contain:

- lifecycle `ledger_head`, revision, generation, attempt id, and fence;
- a new exact candidate id and manifest id;
- deterministic validation result id;
- exact evaluation receipt id and outcome;
- merge or output receipt id;
- cleanup receipt id;
- a `WaitSatisfied` receipt id; or
- for a route breaker only, a successful probe receipt and new breaker epoch.

The following are **not** authoritative progress and never reset backoff:
heartbeats, PID liveness alone, token use, output byte growth, log prose,
`last_interaction_at`, another claim/reservation, another spawn of the same
stage, or another identical infrastructure error. They remain useful evidence
for liveness and diagnosis.

A semantic rejection is authoritative evidence, but its effect is a stage
change to `AwaitSourceRepair`, not another evaluation of unchanged bytes. A
merge conflict similarly changes the next action to source integration. A
changed source candidate creates a new progress stamp and may be evaluated.

## 5. Wakes and one-step rule

The daemon wakes reconciliation on:

- graph/lifecycle/message/worktree-observer events;
- evaluation, disposition, cleanup, process-exit, and route-health receipts;
- the earliest persisted `next_wake_at` deadline;
- daemon restart; and
- a low-frequency safety sweep.

The event loop already combines graph events and a safety timer
(`src/commands/service/mod.rs:3142-3287`, `:3681-3697`). It should include the
earliest durable convergence deadline in the same poll timeout calculation.
Restart loads state, recomputes records from authoritative stores, and queues
o more than the actions that are due.

One pass chooses a deterministic order such as:

```text
(next_wake_at, goal priority descending, task id, stage)
```

It acquires one action lease, rechecks every fence/evidence precondition, takes
**one** idempotent next action, persists the result and next wake, then yields.
Batch throughput comes from repeated passes, not an unbounded pass holding the
graph lock. An event for a record already leased is folded into a pending wake.

## 6. Durable exponential waking

For the same `(goal, generation, stage, blocker class, authoritative progress
stamp)`:

```text
delay = min(cap, base * 2^failures_without_progress)
jitter = hash(jitter_seed, failures_without_progress) mod (delay / 4 + 1)
next_wake_at = observed_at + delay + jitter
```

The seed is persisted, so restart preserves both the deadline and the stagger.
Restart never redraws jitter or resets the exponent. When a stamp advances, the
new stage/blocker starts at its base delay.

Policy classes, with values configuration-controlled rather than embedded in
task status:

| Class | Typical policy |
|---|---|
| local short transient | seconds to minutes, bounded at hours |
| evaluation infrastructure | seconds to minutes; same exact candidate/route |
| route outage | route breaker controls probe cadence; affected tasks add deterministic stagger |
| ancillary cleanup | minutes to hours; semantic disposition remains durable |
| needs-human | event-driven plus a slow hours/day safety probe |

There is no retry-exhaustion transition from a transient infrastructure class
to generic `Failed`. Long falloff keeps the goal live and visible. A truly
semantic terminal failure still follows the explicit lifecycle policy; it is
not inferred from retry count.

## 7. Required convergence cases

| Observation | Correct existing goal/stage | Exactly one next step |
|---|---|---|
| dead owner of a working goal | same task, `ObserveOwner` | run exact-identity triage; sweep only repairs a split-save orphan; preserve source and resume the same task when reasoning is still needed |
| overdue transient retry | same task, `AwaitDispatch` | clear only the elapsed durable deferral and offer the same route/goal to admission |
| satisfied explicit wait | same task, `AwaitWait` | let the wait matcher emit its attempt-bound `WaitSatisfied` receipt and reopen that attempt (`coordinator.rs:606-783`) |
| pending evaluation | same source, `AwaitEvaluation` | run or link one exact-candidate bounded/deep evaluation record; infrastructure sleeps, semantic result redirects |
| accepted but source finish not completed | same task, `AwaitSourceFinish` | notify a live owner, or resume the original goal in its retained source context; daemon does not borrow source promotion authority |
| merge/output receipt exists, cleanup missing | same task, `AwaitCleanup` | call idempotent `cleanup_finish`; only its cleanup receipt authorizes terminal `Done` |
| route outage | every affected task remains at its current semantic stage | defer on one route breaker, acquire one probe lease, then stagger same-route wakes after success/falloff |
| semantic rejection | same task, `AwaitSourceRepair` | wake the original goal with exact rejection evidence; never rerun unchanged bytes and never invent edits |
| real new independent goal discovered | new graph task | create only with an explicit new goal/contract/dependency; bookkeeping is not a goal |

For accepted-not-finished recovery, an old exact-candidate receipt may be reused
only if its candidate, lifecycle tuple, and finish fence are still current. If
not, the resumed source owner must submit a newly bound candidate. The daemon
never infers equivalence.

## 8. Route outage: one breaker, one probe, no fallback

The breaker key is the existing non-secret `HealthRouteKey` of handler, wire,
and endpoint fingerprint (`src/service/provider_health.rs:17-102`). Persist:

```text
RouteBreaker {
  route_id,
  epoch,
  state: Healthy | Unavailable | Probing,
  consecutive_outages,
  next_probe_at,
  probe_lease,
  last_failure_receipt,
  last_success_receipt,
}
```

All outage evidence for the same key joins this record. Exactly one daemon-owned,
credential-bounded probe may hold `probe_lease`; a probe is not a graph task and
has no LLM/controller persona. Other goals sleep until `next_probe_at` plus
their stable jitter. Probe failure advances the route exponent once and
staggered deadlines are recomputed from the new route epoch. Probe success
closes the breaker and emits one authoritative route progress receipt.

The reconciler never changes a task's model, handler, provider, endpoint,
profile, or reasoning to escape an outage. It invokes no cross-model or
cross-route fallback. Existing provider-health/global zero-output mechanisms
may remain evidence producers during migration, but their independently
scheduled global pause/backoff authority must be removed or delegated to this
single route breaker. In particular, the global backoff in
`src/commands/service/zero_output.rs:70-180` and global `service_paused` in
`src/service/provider_health.rs:375-505` cannot coexist as separate outage
controllers after cutover.

Static `coordinator.max_agents`/`runtime_max_agents` remains an admission cap.
Route outage convergence does not require an adaptive parallelism controller.

## 9. Semantic, infrastructure, and human outcomes

- **Transient infrastructure:** preserve the task, attempt evidence, route, and
  source; persist falloff; do not spend a source-quality retry.
- **Evaluation unavailable/insufficient:** remain on the exact candidate and
  evaluation stage; do not report semantic rejection and do not fallback.
- **Semantic rejection:** move to `AwaitSourceRepair`, surface the exact bounded
  evidence, and wake the same goal owner. Deterministic code never edits source.
- **Integration conflict:** move to `AwaitSourceRepair`/integration and wake the
  same worktree. No merge task is created.
- **Needs human:** park the same nonterminal goal with a typed blocker. Message,
  config, credential, route, or graph events wake it immediately; a slow probe
  keeps diagnostics fresh. Silence never turns it into generic `Failed`.
- **Novel work:** a new graph task is allowed only when there is a new
  goal-bearing deliverable with its own completion contract. Evaluation,
  retries, probes, merge, cleanup, status repair, and controller bookkeeping do
  not qualify.

## 10. Migration and duplicate-authority removal

Implementation must be a cutover, not a fifth loop:

1. add the durable read model and derive records without mutation;
2. have restart, graph events, deadlines, and safety sweeps enter one
   `reconcile_due` path;
3. call existing domain-owner functions from that path with explicit one-step
   results and receipt ids;
4. migrate existing respawn/zero-output/provider deadlines into task/route
   records without resetting them;
5. remove independent scheduling authority from reaper/sweep/evaluation/finalize
   replay while retaining those modules as action owners;
6. prove no `.daemon-*` or controller task is created; and
7. only then enable mutation.

The existing service-process supervisor in
`src/commands/service/supervisor.rs` is unrelated: it restarts the daemon
process. Rename it only if necessary for clarity; do not mix its process restart
budget with goal convergence state.

## 11. Validation plan for the future implementation

These checks are intentionally **not** implemented by this decision draft.
They become the hard gate of the operator-approved implementation task.

### Unit/integration

- serialize state, restart the daemon, and prove `next_wake_at`, exponent,
  jitter seed, blocker, and progress stamp are byte-stable;
- prove an identical heartbeat/log/spawn/failure does not reset falloff, while a
  new candidate/verdict/disposition/cleanup/wait receipt does;
- table-test every row in §7 against the existing task id and expected stage;
- crash before and after action-effect/action-receipt/state-save boundaries and
  prove the same action id is applied at most once;
- prove only one probe lease exists per `HealthRouteKey` and N blocked goals get
  distinct deterministic wake times;
- prove semantic rejection selects source repair and never calls a source write;
- assert the coordinator contains one scheduling entry point and existing
  owners have no competing timers.

### Credential-free smokes

1. **restart persistence:** fake-clock a long transient falloff, kill/restart
   `wg service`, and show the original goal remains nonterminal with the same
   deadline/exponent.
2. **case matrix:** seed dead owner, overdue retry, satisfied wait, pending
   evaluation, accepted-not-finished, and merge-receipt-cleanup states; drive
   the real daemon and assert every case advances through the same source task.
3. **route outage:** a fake provider fails all requests; assert one probe,
   staggered wakes, no fallback route, no storm, and recovery after one probe
   success.
4. **semantic rejection:** publish a deterministic reject receipt and assert the
   same goal is awakened in its preserved source context with no daemon source
   edit/new repair task.
5. **long falloff:** advance several transient failures to the cap and assert
   the task is still live/visible rather than `Failed` or `Incomplete`.
6. **absence:** assert no `.daemon-*`, supervisor, probe, merge, repair, or
   cleanup graph row and no controller model invocation.

The smoke manifest must list the approved implementation task as owner. This
draft must not add that manifest entry because no implementation task has been
approved or published.

## 12. Operator decision required

The three older tasks are published and express overlapping authority:

- `impl-supervisor-hard-agent` owns reset/requeue/backoff as a fourth persona;
- `impl-adaptive-parallelism-controller` owns provider-pressure response through
  `max_agents`; and
- `impl-supervisor-controller-composition` deliberately couples the two.

Choose exactly one migration before any new implementation is added or
published.

### Option A — recommended: clean supersession

Abandon all three older tasks (and their pending FLIP satellites) as superseded
by one new `impl-deterministic-convergence-reconciler` task. Keep static
parallelism/runtime pin authority; convergence owns route breaker/probe and all
durable wakes. This is the approved simpler model with the smallest authority
surface.

### Option B — retain budget allocation only

Supersede `impl-supervisor-hard-agent` and
`impl-supervisor-controller-composition`. Rewrite
`impl-adaptive-parallelism-controller` as an orthogonal **budget/admission cap**
only: no task reset, retry/backoff, provider outage, breaker, probe, route
fallback, or reconciler input. Remove its dependency on the supervisor. The
convergence reconciler remains the sole owner of route/task wake policy.

### Option C — edit in place

Rewrite `impl-supervisor-hard-agent` completely into this deterministic daemon
reconciler, abandon the composition task, and narrow the adaptive controller as
in Option B. This avoids a new task id but leaves misleading historical naming
and is therefore not recommended.

After the operator chooses, use explicit task metadata (`wg abandon ...
--superseded-by ...` or a full `wg edit` replacement) and rewire dependencies in
one transaction-shaped maintenance pass. Do not merely add another task beside
the old three. The downstream synthesis must carry this gate until the graph
records the choice.
