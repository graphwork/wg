# WG lifecycle/finish protocol (Lean 4)

This directory checks the correctness-critical **control-state abstraction** of
WG lifecycle and finish. It is intentionally not a model of the operating
system, Git/filesystem implementation, provider execution, or UI.

## Build

```sh
cd formal
lake build
```

`lean-toolchain` pins Lean 4. The project has no package dependencies beyond
Lean's bundled `Std`. CI also rejects `sorry`, `admit`, `unsafe` declarations,
and unscoped `axiom` declarations in checked-in Lean sources.

## Model boundary

`WGLifecycle/Model.lean` defines the version-1 wire state, events, executable
`reduce`, induced `Step`, and `Reachable`. The matching production-facing pure
reducer is `src/lifecycle_protocol.rs`. Effectful process observers,
finalization storage, Git promotion, and cleanup adapters are outside the
model: they may emit an event only after obtaining the corresponding durable
fact, then persist the reducer result atomically. They must not independently
decide a lifecycle edge.

The modeled state includes:

- task phase and `(task, generation, attempt, fence)` capability;
- wrapper/native-child epochs (the wrapper owns the child; it is not a
  descendant of the child);
- attempt, worktree, Pi-session, and finish leases;
- immutable candidate/base-CAS acceptance and the abstract proof that `.wg`
  control-plane resources are excluded from the candidate projection;
- finish transaction, exact successful Land/Deliver/Report receipt, cleanup,
  dependency satisfaction, pending convergence action/deadline, breaker
  charges, and inert messages.

A `Bool` such as `protectedFree`, `promotionReceipt`, or `cleanupCommitted` is
an abstraction of a verified durable receipt, not a claim that a boolean makes
storage durable. `EnvironmentAssumptions` explicitly parameterizes durable
storage, eventual restart, useful fair scheduling, and truthful proven-dead
observation. In particular, fairness must schedule a same-session continuation
or a rank-decreasing finish action; an inert message does not satisfy it.

`NeedsFinalization` is deliberately absent from `Phase`. Settlement or proven
death produces `pending + deadline`. Recovery rank is:

| cut | rank | deterministic next action |
| --- | ---: | --- |
| accepted settlement, no transaction | 3 | `begin_finish` |
| transaction, no disposition receipt | 2 | `promote` |
| receipt, no cleanup | 1 | `commit_cleanup` |
| cleanup committed | 0 | terminal/inert |

## Named theorem modules

- `Safety.lean`: attempt fencing, single ownership, first-terminal-wins,
  immutable exact-candidate/at-most-once promotion, Done/cleanup,
  successful-dependency semantics, wrapper handoff topology, and protected
  control-plane projection.
- `Convergence.lean`: explicit environmental assumptions, non-parking
  finalization, same-session continuation, crash-cut replay/rank descent,
  well-founded recovery, breaker neutrality, and conditional scheduling.
- `Incident.lean`: the exact
  `fix-candidate-wg-control-plane-destruction`, generation 0,
  `attempt-0-1` wrapper/native-child trace. The child settles/exits with no
  finish transaction; the exact owning wrapper creates it and converges once.
- `Golden.lean`: Lean-side executable checks named identically to the committed
  Rust/JSON conformance vectors.

There are no proof placeholders or correctness axioms. Deliberately weakening
stale-capability checks, allowing a second promotion, or allowing terminal
message resurrection invalidates the named proofs and/or golden decisions.

## Wire schema and conformance

The schema version is `1` in both `wireVersion` and
`LIFECYCLE_WIRE_VERSION`. `formal/fixtures/v1/*.json` contains deterministic
wire traces and normalized decisions/states for:

- happy Land, Deliver, and Report;
- the exact production incident;
- stale unrelated caller and protected-resource rejection;
- proven owner death with same-session/worktree continuation;
- lost finish response, CAS movement, and duplicate promotion replay;
- crashes before transaction, after transaction, after promotion, and after
  cleanup;
- terminal message resurrection and breaker-neutral ownership contention.

`tests/lifecycle_protocol_conformance.rs` deserializes every committed fixture
and replays it through the exact reducer compiled from `src/`, then compares
normalized state and every decision. This makes the reducer, not a test-only
mapping, the executable implementation seam.

## Updating model and implementation together

1. Change `WGLifecycle/Model.lean` and `src/lifecycle_protocol.rs` in the same
   commit. Preserve field/event spelling across the JSON wire.
2. Update the named safety/convergence theorem. Never add `sorry`, `admit`, an
   unscoped axiom, or `unsafe` to make a change compile.
3. If the wire meaning changes incompatibly, bump both version constants and
   create a new `formal/fixtures/vN/` directory; never reinterpret old vectors.
4. Regenerate deterministic fixtures:

   ```sh
   UPDATE_LIFECYCLE_GOLDENS=1 cargo test --test lifecycle_protocol_conformance
   ```

5. Mirror new fixture semantics in `WGLifecycle/Golden.lean`, then run:

   ```sh
   (cd formal && lake build)
   cargo test --test lifecycle_protocol_conformance
   cargo fmt --check
   ```

Runtime adapters that consume this reducer must additionally exercise their
real restart/process/storage path in candidate-binary smoke tests. Those tests
are the evidence for OS/storage assumptions; the Lean model does not pretend
to prove them.
