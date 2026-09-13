# Exact Cargo baseline admission — reproduction and diagnosis

Task: `refresh-build-baseline-admission`

## Pre-change reproduction

The production transition was reproduced against base commit
`8aa604d65e4283b330c867e892635dda4afcbcf5` with a candidate binary and a
disposable project/service outside the source worktree's `.wg` directory.
The fixture used a clean, committed one-package Cargo project and this queue:

1. `a-non-rust`: shell `sleep 20`
2. `b-cold-builder`: shell `cargo check && sleep 20`
3. `c-cold-follower`: the same exact Cargo command

The daemon was configured with three process/build slots, an empty dedicated
`cargo_target_root`, zero disk threshold floors, and one exact cold-baseline
reserve. Before the fix, the registry contained only `a-non-rust`; both Cargo
tasks remained open and unattempted. Human `wg service status` reported the
single-baseline-builder wait even though the only live owner was the non-Rust
`sleep` task and it owned no reusable Cargo baseline key. Restarting was not
part of this reproduction.

The committed smoke fixture
`tests/smoke/scenarios/build_baseline_admission_refresh.sh` is the executable
regression for those same conditions. It fails on the base implementation
because the real cold Cargo builder is never admitted beside the generic shell
worker. It also inspects the produced layer manifests rather than inferring
Cargo identity from task completion.

## Diagnosis (stale-readiness hypothesis disproved)

Inspection of the base admission path showed that every coordinator admission
tick already called `target_cache::has_ready_baseline` for the candidate. There
was no daemon-lifetime cached READY boolean to refresh.

The actual fault was the base `live_cold_builders` heuristic in
`disk_sentinel::build_admission_for_source`: it counted every live task whose
coarse class was `BuildCapable`. A non-Rust full/shell task therefore acquired
the global role of "Cargo baseline builder" without an attested Cargo command,
exact cache key, layer manifest, or ownership row. That false owner prevented
the first real Cargo builder from starting, which made restart appear to fix a
readiness problem when registry lifecycle merely removed the false owner.

## Corrected authority and evidence

Admission now derives baseline authority only from an exact reusable Cargo
identity and re-reads immutable manifest plus final `READY` publication state.
A builder fence is scoped to that exact digest. Other cold keys still consume
disk reservation capacity. Missing/incomplete, cold or READY wrong-key,
changed source, changed toolchain, stale PID identity, and failed-but-running
owners remain fail-closed; a proven dead PID releases the fence.

Evidence is exercised by the configured gate:

```text
cargo fmt --check && cargo clippy --locked \
  && cargo test --locked --lib disk_sentinel::tests \
  && cargo test --locked --test integration_dispatcher_config_roundtrip \
  && git diff --check
```

The integration target also runs the registered owned smoke through WG's Rust
subreaper. Its daemon fixture publishes through the production candidate CLI
`wg disk cleanup --execute`, replays that event idempotently, observes the warm
follower without a daemon PID change, and retains a build-heavy third task as
open/unattempted under the disk-capacity bound.
