# Replayable daemon planner and verification boundary

**Status:** v1 pure planner, durable store, replay CLI, formal core, incident
fixtures and candidate-binary replay smoke implemented by
`formalize-daemon-planner-replay`. Adapter cutover is intentionally incremental;
the migration order below prevents two decision authorities from coexisting.

## Decision

Correctness-critical service decisions use one function:

```text
persisted normalized control state
+ one ordered typed observation/effect acknowledgement
+ logical time
    -> normalized next state
     + zero or more explicit idempotent logical effects
     + bounded violation codes
```

The reference implementation is `src/service/planner.rs`. It is pure: no clock,
environment, filesystem, process, socket, Git, provider, model, signal or NFS
access is possible inside `plan`. `PlannerStore` is the adapter boundary. It
persists the redacted decision trace before publishing the rebuildable state
cache or returning effects for execution. `wg service replay <trace>` invokes
only `replay` and therefore has no adapter or external side effects. The
coordinator's ready-but-no-spawn/no-owner/no-admission-wait monitor uses the
same durable store, captures its minimal bundle, and pauses before later
notification, archival or refresh mutation.

This split is preferable to attempting to verify the daemon's current ambient
reads. Lean can prove a deterministic transition system and conditional
liveness. It cannot prove that Linux reported the right PID, NFS made a rename
durable, Git performed a CAS, a provider did not charge twice, or a signal was
delivered. Those remain typed adapter assumptions tested by runtime guards,
fault injection and candidate-binary smokes.

## Wire and redaction

The planner and trace are independently versioned:

- `DAEMON_PLANNER_SCHEMA_VERSION = 1`;
- `DAEMON_TRACE_SCHEMA_VERSION = 1`.

Incompatible meaning requires new constants and a new fixture directory; old
traces are never reinterpreted. All maps use sorted keys and all enums use fixed
snake-case names, so `serde_json::to_vec_pretty` produces stable bytes.

`OpaqueId` accepts only 1–96 ASCII identifier bytes. Arbitrary text, paths,
URLs, endpoint queries, credentials, prompts, model output, logs and file
content have no replay-wire type. Adapters must replace evidence with a bounded
code plus an identifier/digest before calling the planner. This is redaction by
construction, not a regex applied after a secret has entered a trace.

A trace is capped at 256 observations. `PlannerStore` advances the normalized
checkpoint and retains the newest window. Invariant bundles are content-named
and capped at 32 under `.wg/service/replay/`. The regular authoritative trace is
`.wg/service/decision-trace-v1.json`; `.wg/service/planner-state-v1.json` is a
cache that restart rebuilds from the trace.

## Control state and observations

`TaskKey` binds graph, task, generation, attempt and fence. A cross-graph
observation fails closed. Owner observations distinguish:

- `AuthenticatedLive(actor, lease)`;
- `ProvenDead(actor, lease)`;
- `Unauthenticated(actor)`; and
- `None`.

Only the first is a live owner. PID existence alone must never be translated to
it. Process topology, wrapper/native-child relationship, worktree/session
leases and broker capabilities are adapter evidence that must authenticate the
same tuple first.

An unfinished task must have **exactly one** forward class:

1. runnable action;
2. authenticated live owner;
3. explicit external wait condition; or
4. scheduled convergence action and logical deadline.

Zero classes is `NoForwardDisposition` (the old “no blockers” stall). Multiple
classes is `MultipleForwardDispositions` (the source of reopen/park overlap
races). Corrected rules normalize either case to one immediate
`FailClosedHold`; historical rules expose the violation without repair.

Observations are ordered envelopes `(sequence, logical_time, observation)`.
Exact duplicate sequences are inert, conflicting duplicates and unseen
regressions hold. Acknowledgements may arrive after restart, duplicate, or be
reordered. An early acknowledgement is retained until its logical effect is
observed.

## Logical effects and crash protocol

Effect identity hashes the graph/task/generation/attempt/fence/action/issue
epoch tuple. It never includes wall time or random data.

The adapter protocol is:

1. normalize evidence;
2. `PlannerStore::apply(observation)`;
3. persist the trace (authoritative issue boundary);
4. persist the normalized state cache;
5. return the effect to the adapter;
6. execute the physical operation idempotently using `effect_id`;
7. submit `EffectAcknowledged(effect_id, outcome)` through the same store.

A crash after step 3 replays the same effect ID. A lost response cannot create a
second logical spawn, promotion, archive or chat. `Retryable` may request
another physical execution with the same ID while the logical effect map stays
cardinality one. `Succeeded` and `RejectedStale` settle it. This is logical
exactly-once with physical at-least-once/idempotence; it does not claim the OS or
provider is exactly-once.

The existing worker broker (`worksgood-worker-control-v1`) maps directly to this
boundary: validated capability tuples become task observations, request journal
`Pending` is an explicit reconciliation wait, `Completed(response)` is an
effect acknowledgement, and a fresh request becomes a typed effect. Broker
files must not be read from inside `plan`.

## Invariant monitor

`plan_guarded` computes the prospective corrected transition. If it contains a
violation, it atomically writes the minimal one-observation replay bundle **from
the pre-mutation state** before returning the fail-closed transition. Failure to
write the bundle is an error, so an adapter cannot continue mutation without
the diagnostic. `PlannerStore::apply` calls this guard before changing the
regular trace/cache.

The monitor records codes and identities only. The operator can run:

```sh
wg service replay .wg/service/replay/violation-<digest>.json
```

and obtain the same normalized state, effect IDs and violations byte-for-byte.

## Formal connection

`formal/WGLifecycle/DaemonPlanner.lean` extends the lifecycle/finish model with:

- the four-way forward-exhaustiveness predicate;
- fail-closed normalization for malformed unfinished states;
- exhaustive corrected projection for all nine incident codes;
- duplicate acknowledgement idempotence and effect-ID preservation; and
- conditional liveness under an explicit useful-scheduling assumption.

`formal/WGLifecycle.lean` imports it, so `lake build` checks the extension with
no `sorry`, `admit`, `unsafe` declaration or correctness axiom. Rust fixtures in
`formal/fixtures/daemon/v1/` are replayed by
`tests/daemon_planner_conformance.rs`. The Lean theorem is intentionally about
the normalized transition schema; adapter truth remains outside its claim.

## Permanent incident corpus

| Incident | Historical violation | Corrected unique class |
|---|---|---|
| exited wrapper rejected stale, then `NeedsFinalization` stalls | `ExitedWrapperRejectedStale` | scheduled same-session continuation |
| reopen before old owner release | `ReopenBeforeOwnerRelease` / overlapping classes | scheduled owner release |
| park/resume overlap | `ParkResumeOverlap` / overlapping classes | correlated external wait |
| obsolete daemon chat create, response lost | `ObsoleteDaemonChatCreationLostResponse` | request-journal reconciliation |
| finish target moved | `TargetMovedDuringFinish` | replan against new CAS; never promote stale base |
| surprise archival backlog | `SurpriseArchivalBacklog` | digest-pinned human confirmation wait |
| candidate replaces `.wg` control plane | `ControlPlaneCandidateReplacement` | fail-closed hold |
| dead Pi owner retains session/worktree | `DeadPiOwnerRetainingLeases` | scheduled exact-owner release |
| abandoned dependency satisfies readiness | `AbandonedDependencySatisfiedReadiness` | dependency-success wait |

Each JSON fixture runs once under `Historical` and once under `Corrected`.
Historical replay must contain its named violation; corrected replay must have
exactly one class and the expected effect/wait.

## Verification pyramid

1. **Lean:** forward safety, effect acknowledgement idempotence and conditional
   useful-scheduling liveness.
2. **Bounded/property:** Rust exhaustively enumerates all four class bits for two
   tasks/two attempts and tests stale/current identities, crash, duplicate and
   reordered acknowledgements, and target movement.
3. **Conformance:** permanent incident JSON is replayed through production Rust;
   repeated reports are byte-identical.
4. **Synchronization:** add Loom only at an adapter CAS/lock boundary; the pure
   single-threaded reducer needs no Loom model.
5. **Candidate binary:** `daemon_planner_replay.sh` drives the real CLI, compares
   two reports, verifies graph bytes are unchanged and rejects content-bearing
   output. Effect adapters require their own kill-at-boundary smokes during
   cutover.
6. **Production:** `PlannerStore` persists every normalized decision;
   `plan_guarded` captures violations before the hold transition.

## Adapter cutover plan

Cut over one authority domain at a time. In each step remove the old branch/timer
in the same change; do not leave a planner and legacy loop both issuing effects.

1. dispatch/admission, route breaker and dependency readiness;
2. attempt/process/worktree/session ownership and worker-control requests;
3. wait/message consumption and Pi same-session continuation;
4. finish/promotion/cleanup and target-CAS reconciliation;
5. chat-create request journal, archival hold/confirmation and service
   upgrade/restart migration.

For each domain, first add a read-only normalizer and historical incident trace,
then switch issue/ack persistence, then delete legacy authority. Required
adapter evidence includes source and quality (`authenticated`, `proven dead`,
`durable receipt`, `ambiguous`); an adapter may never collapse unknown to true.
Every effect adapter must fault-inject crashes before issue persistence, after
issue/before execution, after execution/before acknowledgement, and after
acknowledgement. Assertions are no duplicate logical effect, idempotent physical
retry, no lost worktree/session WIP, and no stale/cross-graph mutation.

## Explicit non-goals

The model does not formalize or claim correctness for TUI rendering, model
output, provider behavior, operating systems, PID reuse, signal delivery, Git,
NFS, filesystem durability, sockets, wall-clock accuracy or performance. Those
surfaces are adapters with runtime evidence and tests.
