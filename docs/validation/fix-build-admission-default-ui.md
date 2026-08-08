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

## Full-suite baseline comparison

A serialized full-suite run with worker-control and `WG_GLOBAL_DIR` variables removed (`cargo test -- --test-threads=1`) passed the main binary's 3,790 tests, then reached one pre-existing integration failure: `defaults_no_user_config_are_graph_only_and_unselected` expects an `owner = "Pi"` line that the baseline formatter no longer emits. Running that exact test from a detached worktree at the task's integrated-main commit `da286458` produces the identical failure (`0 passed; 1 failed`).

Continuing with that known test skipped reached three pre-existing `integration_cli_workflows` failures where legacy tests call `wg done` without the now-required completion candidate. The entire baseline integration binary at `da286458` produces the identical three failures (`69 passed; 3 failed`): `test_done_via_cli`, `test_fail_retry_lifecycle_via_cli`, and `test_retry_lifecycle_fail_retry_claim_done`.

These baseline comparisons are reproducible with:

```sh
git worktree add --detach /tmp/wg-baseline da286458
CARGO_TARGET_DIR="$PWD/target-baseline" cargo test --manifest-path /tmp/wg-baseline/Cargo.toml \
  --test integration_canonical_config defaults_no_user_config_are_graph_only_and_unselected -- --test-threads=1
CARGO_TARGET_DIR="$PWD/target-baseline" cargo test --manifest-path /tmp/wg-baseline/Cargo.toml \
  --test integration_cli_workflows -- --test-threads=1
```

No admission-related test failed. Required targeted tests and both owned deterministic smoke scenarios were rerun after the final changes and pass.
