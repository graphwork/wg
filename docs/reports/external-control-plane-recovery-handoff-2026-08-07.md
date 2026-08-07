# External WG control-plane recovery handoff

**Date:** 2026-08-07  
**Execution rule:** do not use the WG daemon/task graph to perform this recovery. Work from plain Git with one independently supervised coding session at a time.  
**Frozen baseline:** `f026b808` (`docs: audit control-plane authority deletion map`)

## Why recovery is external

WG is currently unable to supervise its own repair reliably. Observed failures include live workers declared dead, terminal attempts projected as running, configured timeouts not enforced, current-attempt token usage hidden by prior-attempt totals, completion review exceeding provider context, synthetic assignment tasks blocking source completion, successful recurring shell work revived as running, and provider exits after durable work being recorded as task failure.

The service is intentionally stopped. Do not restart it until the final independent canary passes.

## Source-of-truth documents

Read in this order:

1. [`docs/research/wg-control-plane-authority-deletion-map.md`](../research/wg-control-plane-authority-deletion-map.md) — current authority/store/process inventory and deletion map.
2. [`formal/README.md`](../../formal/README.md) — exact proof boundary and production conformance seams.
3. [`formal/WGLifecycle/Convergence.lean`](../../formal/WGLifecycle/Convergence.lean) — deterministic finish convergence and conditional liveness theorems.
4. [`formal/WGLifecycle/Safety.lean`](../../formal/WGLifecycle/Safety.lean) — fencing, first-terminal-wins, promotion, cleanup, and protected-state invariants.
5. [`formal/WGLifecycle/Incident.lean`](../../formal/WGLifecycle/Incident.lean) — exact incident convergence and non-stuck trace.
6. [`formal/WGLifecycle/DaemonPlanner.lean`](../../formal/WGLifecycle/DaemonPlanner.lean) — unfinished-work exhaustiveness and useful scheduling boundary.
7. [`docs/plans/simple-worker-owned-lean-convergence.md`](../plans/simple-worker-owned-lean-convergence.md) — the previously completed boring worker-owned completion model and clean-room canary.
8. [`docs/plans/deterministic-convergence-final-cutover.md`](../plans/deterministic-convergence-final-cutover.md) — records that the older planner/convergence authority cutover was incomplete.
9. [`docs/research/traces/wg-control-plane-authority-deletion-map/`](../research/traces/wg-control-plane-authority-deletion-map/) — eight bounded production-incident traces.

## What the Lean work actually proves

The Lean work is real and must not be discarded.

Named results include:

- `every_finish_crash_cut_replayable`
- `deterministic_finish_plan_converges`
- `deterministic_same_session_continuation`
- `rank_decreasing_recovery_is_well_founded`
- `conditional_convergence`
- `incident_converges_exactly_once`
- `motivating_trace_not_stuck`

The proof boundary is conditional and explicit: durable persistence, eventual compatible restart, fair scheduling of a **useful** pending action, and truthful proven-dead evidence are adapter obligations. Lean proves the pure reducer preserves safety and that useful recovery strictly descends its rank. It does not prove that Linux, Git, the daemon, providers, observers, or the current collection of adapters actually supply those assumptions.

That distinction explains the current failure: the proven reducer exists, but production still contains overlapping writers, schedulers, journals, and terminal paths that bypass or compete with it.

## Recovery decision

Do **not** invent a new convergence protocol. Preserve the proven semantic core and make it the only finish/attempt decision boundary. Delete or demote every competing mechanism to an evidence adapter or read-only projection.

Target runtime:

1. one durable attempt/recovery reducer, derived from the proven model;
2. one exact process-tree supervisor;
3. one normalized observation stream;
4. one resumable completion transaction;
5. one wrapper process per worker;
6. graph, registry, telemetry, TUI, and logs as projections only.

Keep immutable completion-v3 evidence and review receipts. Keep direct readiness/`SpawnPlan` calculation. Keep the useful journal-integrated resume logic as an adapter. Remove its ability—and every other adapter's ability—to independently alter task lifecycle.

## Independent implementation sequence

Use a plain Git branch from `f026b808`. Each step must delete the old issuer in the same commit that routes its behavior through the retained reducer. No dual authority and no compatibility controller.

### 1. Re-establish formal/Rust equivalence

- Run the existing Lean build and conformance tests before changing production.
- Identify the exact production reducer corresponding to each theorem and fixture.
- Add characterization only where the production seam has drifted; do not add a second reducer.

### 2. One terminal path

- Route completion-v3 publication receipt, explicit failure, park/wait, cancel, and exact process exit through the retained reducer.
- Remove every direct task-status writer listed in audit §7.
- A terminal attempt cannot reopen. A later run requires one explicit next-attempt operation.

### 3. One process supervisor and observation lane

- The wrapper owns the exact child process tree and emits start/exit observations.
- Heartbeats, stream timestamps, worktree hashes, provider events, and compaction events are evidence only.
- Delete heartbeat/reaper transition authority, always-on worktree observer control state, Pi watchdog transition state, and duplicate terminal fallbacks.

### 4. One completion transaction

- Keep completion-v3 CAS objects and manifest-bound review receipts.
- Fold the minimum crash-resumable publication state into the attempt journal.
- Migrate and delete SaveTransaction v2, Finalization v1, legacy `done`, and convergence finish transaction state.
- Internal assignment/review bookkeeping is an audit receipt, never a graph task or dependency.

### 5. Delete the separate schedulers

- Remove PlannerStore scheduling authority, mutable `ConvergenceState`, independent retry/replay timers, synthetic `.assign-*`/`.flip-*`/`.evaluate-*` tasks, and the fifteen compatibility paths in audit §8.
- Preserve convergence semantics inside the single reducer; delete the separate converger controller and state file.

### 6. Projection repair

- Show current-attempt usage from the current exact stream and cumulative usage separately.
- Make task status, registry state, TUI state, provider health, and telemetry rebuildable from the attempt journal plus immutable evidence.
- Retain the last valid TUI snapshot on schema error.

### 7. Independent installed canary

Start the service only inside an isolated temporary `HOME`/graph. After start, issue no operator mutations. The canary must cover:

- dependencies and normal worker graph coordination;
- assignment and manifest-bound review without synthetic task dependencies;
- exact worktree retry;
- daemon crash at every reducer/effect boundary;
- duplicate/reordered observations and lost responses;
- live PID with missing heartbeat;
- exact process loss;
- recurring shell completion and schedule advancement;
- repeated qualifying Pi compactions;
- completion publication followed by wrapper/provider failure;
- current-attempt and cumulative accounting.

Every goal must end in a valid terminal state or an explicit external wait. No task may remain running without a live exact owner or a durable useful pending action. No operator retry, graph edit, kill, or manual finalization is allowed.

## Required commands

Run focused tests while editing. At formal checkpoints:

```bash
(cd formal && lake build && lake build simple-land-oracle)
! grep -RInE --include='*.lean' '(^|[^[:alnum:]_])(sorry|admit)([^[:alnum:]_]|$)|^[[:space:]]*(unsafe|axiom)[[:space:]]' formal
cargo test --locked --test lifecycle_protocol_conformance -- --test-threads=1
cargo test --locked --test daemon_planner_conformance -- --test-threads=1
cargo test --locked --test save_transaction_conformance -- --test-threads=1
cargo test --locked --test simple_land_conformance -- --test-threads=1
cargo test --locked --test simple_land_lean_oracle -- --test-threads=1
```

Only after the implementation is deletion-complete:

```bash
cargo fmt
cargo fmt --check
cargo clippy
cargo install --path . --locked
```

Then run the one isolated installed canary. Do not use the historical 177-target integration invocation during development.

## Quantitative merge gate

Reject the recovery if it adds a new authority or fails to reduce the audit baseline:

```text
authority mechanisms: 22 -> lower, ultimately 1 semantic kernel
production control-plane LOC: 52,304 -> materially lower
status writers outside projection applier: 32 -> 0
independent reducer families: 9 -> 1
durable control-store families: 18 -> 1 authority + evidence stores
append/replay journals: 11 -> 1 semantic journal
per-attempt helper roles: 7 -> 1 wrapper
compatibility paths: 15 -> 0 after one migration
```

A change that merely makes an incident green by adding another phase, journal, helper, retry lane, or compatibility reader is rejected.

## Current preserved work

- Audit landed on `main`: `f026b808`.
- Pi/reviewer experimentation remains preserved on `wg/agent-13/implement-wg-pi-compaction-kick`; do not merge wholesale.
- The first post-audit implementation attempt is preserved in `.wg-worktrees/agent-31` but is uncommitted, does not compile, and is net `+921/-178`. Treat it as a source of individual test ideas only, not as a candidate.
- The previous bounded integration-harness/storage prototype is preserved on its branch; extract only the harness consolidation separately if required for validation.

## Restart criterion

Do not restart WG for self-hosted development. Restart the installed daemon only after the isolated external canary passes and the quantitative merge gate confirms that control-plane authority decreased rather than moved or multiplied.
