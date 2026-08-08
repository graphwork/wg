# Validation: inherited build admission capacity

Validation run in the task worktree on 2026-08-08 with the pinned Rust 1.96.0 toolchain.

## Required gates

| Command | Result |
|---|---|
| `cargo build --bins --quiet` | PASS (exit 0; existing warnings only) |
| `cargo fmt --check` | PASS (exit 0, no output) |
| `cargo clippy --quiet` | PASS (exit 0; existing warnings only) |
| `git diff --check` | PASS (exit 0, no output) |
| `bash -n tests/smoke/scenarios/admission_deferral_backpressure.sh tests/smoke/scenarios/build_admission_inherits_worker_slots.sh` | PASS (exit 0, no output) |
| `cargo install --path . --locked` | PASS (release binaries replaced) |

## Targeted tests

- `cargo test --quiet build_heavy_capacity_inherits_worker_slots_and_preserves_explicit_override` — PASS: 1 passed, 0 failed.
- `cargo test --quiet admission_deferral_never_becomes_spawn_failure_or_pending_eval` — PASS: 1 passed, 0 failed.
- Existing targeted config/coordinator/TUI tests executed during implementation passed.

## Live deterministic scenarios

Both commands ran with worker-control environment variables unset and the candidate binaries first on `PATH`.

- `tests/smoke/scenarios/build_admission_inherits_worker_slots.sh` — **PASS**: fresh generators omit the cap; inherited and explicit capacities hot-reload; exact inheritance remediation works.
- `tests/smoke/scenarios/admission_deferral_backpressure.sh` — **PASS**: live daemon reports admission backpressure, coalesces it beyond five ticks, and launches the deferred build exactly once after capacity frees. The scenario drives a real tmux TUI and asserts CLI, JSON, worksgood, dashboard, task-inspector, no-attempt neutrality, and disabled-sentinel output.

## Full-suite context

A full `cargo test` run during implementation completed 3,111 tests successfully and reported 7 existing `profile::named` failures caused by parallel mutation of shared profile state. The root failing profile test passed when rerun alone. No admission-related test failed. The required targeted tests and owned deterministic smoke scenarios above were rerun after the final repair changes and pass.
