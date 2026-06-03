# Autopoietic loop failure integration

Date: 2026-06-03
Task: `integrate-autopoietic-loop-failure`
Upstream fix task: `fix-autopoietic-loop-failure`

## Decision

The WG service/executor fix from `fix-autopoietic-loop-failure` is integrated.
Main already contains the reviewed worker diff as squash commit `ba48a779`
(`feat: fix-autopoietic-loop-failure (agent-73)`). This integration branch also
merged the original worker commit `6b8e9e0c` as merge commit `70ed90fa` so the
worker-output provenance remains visible to WG evaluation and downstream users.

The root-level report files remain available in the repository:

- `wg-autopoietic-loop-failure-report-20260603.md`
- `wg-autopoietic-loop-failure-triage-20260603.md`

No failed or abandoned C4 scientific recovery tasks were resumed, dispatched,
edited, marked terminal, or otherwise mutated during this integration. A graph
check found no current entries matching `autopoietic-c4-compression`,
`c4-loop-01` through `c4-loop-04`, `c4-autopoietic-recovery`, or
`c4-blocker-01-poasta-scale-impl`.

## Coverage

The integrated fix includes regression coverage for the WG behavior reported in
the incident:

- Codex optional tool/user config startup failure classification:
  `tests/integration_failure_classification.rs`
  `test_classify_failure_subcommand_codex_unavailable_optional_tool_model`
- Blocked-open dependency cycle diagnostics:
  `tests/integration_cycle_detection.rs`
  `test_failed_root_blocked_open_cycle_gets_readiness_diagnostic`
- User-facing `wg ready` blocked-open cycle diagnostic:
  `tests/integration_cycle_detection.rs`
  `test_wg_ready_surfaces_failed_root_blocked_open_cycle_diagnostic`
- Service-visible Codex spawn isolation smoke:
  `tests/smoke/scenarios/codex_optional_tool_config_ignored.sh`

The smoke manifest lists `fix-autopoietic-loop-failure` as an owner for
`codex_optional_tool_config_ignored`.

The supervisor/process-owner versus docs-only evaluator path was not changed in
the integrated fix because triage could not recover the historical task
metadata. It is split into the sequential follow-up
`fix-supervisor-loop-evidence`, with validation requiring deterministic routing
or evaluation coverage proving that supervisor/process-owner tasks cannot be
accepted as docs-only reports when implementation subtasks or progress artifacts
are required.

## Validation

Passed:

- `tests/smoke/scenarios/codex_optional_tool_config_ignored.sh`
  - Result: PASS
  - Evidence: fake Codex worker accepted the real dispatcher spawn only when
    WG passed `--ignore-user-config`.
- `cargo test --test integration_failure_classification
  test_classify_failure_subcommand_codex_unavailable_optional_tool_model -- --nocapture`
  - Result: PASS
- `cargo test --test integration_cycle_detection
  test_failed_root_blocked_open_cycle_gets_readiness_diagnostic -- --nocapture`
  - Result: PASS
- `cargo test --test integration_cycle_detection
  test_wg_ready_surfaces_failed_root_blocked_open_cycle_diagnostic -- --nocapture`
  - Result: PASS
- `CARGO_INCREMENTAL=0 cargo build`
  - Result: PASS after cleaning this worktree's `target/`.
  - Note: an initial plain `cargo build` attempt failed only because Cargo could
    not write an incremental query cache with `No space left on device`.
- `wg config lint`
  - Result: PASS
  - `/home/bot/.wg/config.toml` and `/home/bot/wg/.wg/config.toml` were clean.
- `CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0' cargo install --path . --locked`
  - Result: PASS
  - Replaced global `wg` and `nex` from this worktree.
- Existing daemon observation
  - Result: PASS for two-plus poll intervals.
  - The daemon was already running, so this integration did not start or stop it.
  - Observed tick advancement from `#108` to `#117` with `poll_interval=5s`,
    `tasks_ready=0`, and `spawned=0`.

Attempted but not completed:

- Full `cargo test`
  - Attempt 1: `CARGO_INCREMENTAL=0 cargo test`
  - Attempt 2: `CARGO_INCREMENTAL=0 RUSTFLAGS='-C debuginfo=0' cargo test -j1`
  - Result: both attempts failed during integration-test binary linking under
    filesystem pressure. The first run reported `No space left on device` and
    linker bus errors; the serial no-debug-info retry still failed linking
    `integration_dispatch_boot` with `ld` bus error. No Rust test assertion
    failure was reached in either attempt.

## Hygiene

Only task-relevant source/test/report changes are part of the integration
branch. The only untracked path observed before staging was `.wg`, a worktree
metadata symlink to `/home/bot/wg/.wg`; it was not staged or committed.

No API keys, tokens, secret-bearing environment dumps, or unrelated files were
printed, staged, or committed.

## Residual risk

The remaining risk is environmental: the full test suite could not complete in
this worktree because the shared filesystem was near capacity and the large WG
integration-test binaries failed during linking. The targeted regressions for
the incident, the service-visible smoke scenario, `cargo build`, config lint,
and install all passed.
