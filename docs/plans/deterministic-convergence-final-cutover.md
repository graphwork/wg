# Deterministic convergence reducer — final authority cutover

**Status:** approved implementation plan. The pure reducer and replay boundary exist, but production adapters have not completed the authority cutover.

## Current gap

`src/service/planner.rs` is currently authoritative only for replay fixtures, failed-prerequisite decisions, and the dispatch exhaustiveness alarm. Normal production scheduling still runs through a second reducer (`src/service/convergence.rs`) plus direct mutation/timer lanes in coordinator startup/ticks, triage, orphan sweep, reopen, zero-output handling, provider health, waits, finalization replay, cleanup, chat creation, and archival.

The final implementation must make `PlannerStore` the only component allowed to issue a correctness-critical logical effect. Domain adapters may observe external state and execute a planner effect, but may not independently decide that the effect is due.

## Definition of done

1. Every unfinished task has exactly one planner projection: runnable effect, authenticated live owner, explicit external wait, or scheduled effect with logical deadline.
2. Every correctness-critical effect is persisted before execution, carries a stable `effect_id`, and is acknowledged through `PlannerStore` after execution.
3. Restart replays the same effect ID; duplicate/reordered acknowledgements are inert.
4. One event loop deadline is derived from planner state. No second retry, breaker, replay, reaper, wait, cleanup, archive, or migration timer has decision authority.
5. Provider health and zero-output detection emit typed observations only. They cannot pause the whole service, fail/reopen a task, choose a route, or schedule a retry directly.
6. Triage, worktree/session inspection, Git, process, sockets, and provider calls remain untrusted adapters. Unknown evidence never becomes `AuthenticatedLive`, `ProvenDead`, success, or rejection.
7. SaveTransaction/finalization progression is issued one phase at a time by planner effects. A terminal intent cannot revoke the exact owner's ability to settle it, and no wrapper/provider exit can overwrite durable success.
8. `convergence-state.json` and the mutable scheduling authority in `src/service/convergence.rs` are migrated once and then removed. Status may retain a read-only projection, but not a second state machine.
9. No daemon/controller/probe/repair/merge/cleanup graph task is created for bookkeeping.
10. Lean/Rust traces, fault injection, and candidate-binary smokes cover every cutover domain with no proof escapes.

## Ordered cutover

Cut over one authority domain at a time. Each change must remove the corresponding legacy decision branch in the same commit; dual authority is forbidden.

### 1. Runtime kernel and migration

- Add production observation normalization and an effect-execution journal around `PlannerStore`.
- Add stable sequence allocation, logical time, issue-before-effect, acknowledgement, restart replay, and schema migration.
- Import existing durable convergence deadlines/backoff/route state without resetting them.
- Expose a read-only status projection and one earliest planner deadline.
- Do not cut over a domain yet.

### 2. Dispatch, dependency readiness, and route health

- Normalize ready/dependency/admission/route observations into the planner.
- Planner effects become the only source of spawn and route-probe authorization.
- Convert resource pressure to an explicit wait observation.
- Remove `admit_goal_action`, `admit_route_action`, independent spawn-falloff mutation, global provider pause, and zero-output circuit-breaking authority.
- Preserve exact route/model; no fallback.

### 3. Attempt, process, worktree, session, and capability ownership

- Normalize exact tuple-authenticated owner evidence.
- Planner effects become the only source of dead-owner release, same-session resume, stale-owner reclaim/retain, and orphan reconciliation.
- Triage/reopen/sweep/watchdog/capability code executes effects only.
- Remove tick-ordered direct mutation chains that can race one another.

### 4. Wait/message and Pi continuation

- Normalize correlated waits/messages and session continuation evidence.
- Planner effects become the only source of message consumption, wait satisfaction, reopen, and same-session continuation.
- Messages for stale/non-waiting attempts remain inert.
- Remove independent wait polling/resume authority.

### 5. SaveTransaction, evaluation, promotion, and cleanup

- Normalize exact SaveTransaction/finalization/evaluation/CAS receipts.
- Planner effects advance exactly one phase: quiesce, WorkSave, seal, validate/evaluate, disposition, promotion/output, cleanup, GraphSave projection.
- Startup replay and exited-worker convergence become effect executors, not schedulers.
- Distinguish semantic source repair from infrastructure insufficiency and operator reconciliation.
- Delete direct startup/tick finalization replay authority.

### 6. Chat creation, archival, and service migration

- Move chat request-journal reconciliation, archive confirmation/batches, and service-state migration under planner effects.
- Automatic archival remains opt-in; overdue/new-build backlog requires digest-pinned confirmation.
- Lost IPC responses reconcile by request ID.
- Remove their independent daemon timers/decision branches.

### 7. Legacy deletion and final proof

- Delete mutable `ConvergenceState` scheduling and migrate/remove `convergence-state.json`.
- Assert there is one production scheduling entry point and one logical deadline source.
- Add permanent incident traces and crash points: before issue persistence, after issue/before execution, after execution/before acknowledgement, and after acknowledgement.
- Run Lean, Rust conformance/property tests, route outage, dead owner, wait, source repair, accepted-not-finished, cleanup, chat, archive, and long-falloff candidate smokes.

## Mandatory validation for every cutover

- `cargo fmt --check`
- `cargo clippy --locked`
- focused unit/property/conformance tests
- domain candidate-binary smoke from a clean graph and isolated `HOME`
- restart at every effect boundary; stable effect ID and no duplicate physical consequence
- no source/worktree/session loss and no stale/cross-graph mutation
- existing planner trace replay remains byte-identical

The final synthesis additionally runs `lake build`, scans for `sorry`/`admit`/unsafe proof escapes, and proves that no legacy authority can issue a scheduled mutation without a planner effect.
