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
- `cargo test --quiet unavailable_service_clears_admission_and_reloads_capacity` — PASS: 1 passed, 0 failed.
- `cargo test --quiet unavailable_process_identity_is_explicitly_unknown_and_fails_closed` — PASS: 1 passed, 0 failed.
- `cargo test --quiet authoritative_service_identity_click_keyboard_and_stale_coordinate_parity` — PASS: 1 passed, 0 failed (including physical Shift+I parity).
- Existing targeted config/coordinator/TUI tests executed during implementation passed.

## Live deterministic scenarios

Both commands ran with worker-control environment variables unset and the candidate binaries first on `PATH`.

- `tests/smoke/scenarios/build_admission_inherits_worker_slots.sh` — **PASS**: hot reload exact-once, scoped remediation, stopped freshness, and launch-time runtime-pin CLI/TUI parity pass.
- `tests/smoke/scenarios/admission_deferral_backpressure.sh` — **PASS**: live admission UI and exact-once work pass; hard-crash/PID-reuse orphaned deferrals are suppressed across CLI/TUI (including a live unrelated PID with a mismatched OS birth token). The scenario drives a real tmux TUI and asserts CLI, JSON, worksgood, dashboard, task-inspector, no-attempt neutrality, and disabled-sentinel output.

## Scope

The task's required validation gates are the targeted tests, formatting/lint checks, and owned deterministic smoke scenarios recorded above. All were rerun after the final changes and pass.
