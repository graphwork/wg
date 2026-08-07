# Control-plane recovery: corrected baseline and proof boundary

**Branch baseline:** `f026b808`  
**Execution mode:** external, one plain-Git coding session; WG service remains stopped

## Baseline validation

The required pre-change checks pass:

- `(cd formal && lake build && lake build simple-land-oracle)`
- no `sorry`, `admit`, `unsafe`, or unscoped `axiom` in checked-in Lean
- `lifecycle_protocol_conformance`: 5 passed
- `daemon_planner_conformance`: 8 passed
- `save_transaction_conformance`: 2 passed
- `simple_land_conformance`: 2 passed
- `simple_land_lean_oracle`: 1 passed
- eight credential-free deletion-audit traces replay to their recorded target projections

## Corrected quantitative baseline

The deletion audit's counting method truncated every Rust file at its first
`#[cfg(test)]`. That is not a safe production boundary: files such as
`src/commands/abandon.rs`, `src/commands/done.rs`, `src/commands/exec.rs`, and
`src/commands/service/coordinator.rs` contain a test-only import near the top and
production code after it. Those production writers were omitted.

`scripts/control_plane_metrics.py` masks the individual cfg(test)-annotated item
instead. It also scans status authority repository-wide rather than limiting the
writer scan to the fixed LOC manifest.

Corrected frozen baseline (`docs/reports/control-plane-recovery-baseline-corrected.json`):

```text
fixed control-plane paths:        62
production control-plane LOC:     71,678
status writers outside applier:   65 in 26 files
```

The original `52,304` and `32 in 20 files` remain useful historical lower bounds,
but they must not be used as the recovery merge gate. The corrected numbers are
the no-accretion baseline.

## Exact proof-to-production map

There is no single existing reducer that is both the named proven model and the
sole production decision boundary.

| Formal surface | Rust executable | Production status |
|---|---|---|
| `WGLifecycle.Model`, `Safety`, `Convergence`, `Incident` | `src/lifecycle_protocol.rs` | Reference reducer. The narrow `service::convergence::reduce_exited_worker_finish` projection is production-reachable. |
| `WGLifecycle.V2` | `src/save_transaction.rs` | Production/compatibility transaction targeted for migration and deletion. |
| `WGLifecycle.SimpleLand` | `src/simple_land.rs` | Lean/Rust equivalent, but `reduce_simple_land` has no production caller. Completion commands duplicate its checks. |
| `WGLifecycle.DaemonPlanner` | `src/service/planner.rs` | Replay/store model; direct dispatch explicitly does not use PlannerStore. Targeted for deletion. |
| — | `src/lifecycle.rs::LifecycleKernel` | Broad production lifecycle reducer, but not the exact named formal reducer and bypassed by direct writers. |

Therefore recovery must derive the reduced in-place attempt kernel from the proven
invariants. It must not claim that current Lean/Rust equivalence already covers the
production lifecycle.

## Retained semantic obligations

The reduced kernel must preserve:

1. exact attempt/fence checking;
2. identical idempotency key plus digest returns the original response;
3. conflicting reuse is rejected;
4. first terminal intent wins;
5. success requires an exact immutable completion/publication receipt;
6. observations cannot infer success or terminalize directly;
7. exact exit without terminal intent issues one stable reconciliation effect;
8. terminal attempts never reopen without an explicit next-attempt request;
9. review and publication remain bound to one immutable manifest;
10. graph, registry, telemetry, and TUI are rebuildable projections.

## Recovery result

At the final recovery cut, the same scanner reports:

```text
fixed control-plane paths:        62
production control-plane LOC:     67,991
status writers outside applier:   0 in 0 files
```

The recovery deleted 3,687 production control-plane lines (5.1%) while removing
all measured direct task-status assignments outside the lifecycle applier.
Completion-side validators, evaluator reconciliation, heartbeat age, daemon
cleanup, replay, IPC, cron, and remote-result observations now either emit
receipts or request lifecycle transitions; they no longer project task status
independently.

This metric establishes the production authority boundary, not formal
correctness by itself. The formal builds, conformance suites, and independent
canary remain separate acceptance evidence.
