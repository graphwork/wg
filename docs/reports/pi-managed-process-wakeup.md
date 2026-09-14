# Pi managed-process yield/wake compatibility proof

## Finding

**Supported, narrowly and opt-in:** a WG Pi task may set
`WG_PI_PROCESS_WAKE_EXTENSION` to the absolute entry file of an isolated,
locally installed `@mjakl/pi-processes@2.0.0`. The production spawn builder then
uses WG's hidden `pi-process-worker` adapter instead of the default one-shot
worker command. The adapter runs real `pi --mode rpc`, explicitly loads exactly
the embedded WG extension and the supplied process extension under `-ne`, and
keeps one Pi OS process/session alive until the extension's registered
`pi-processes:update` event has driven its continuation turn.

This does **not** change the default. Without the environment variable, Pi task
workers remain `pi --mode json -p`; WG chat continues to use its existing RPC
handler.

## Exact tested artifacts

- Pi CLI: `0.84.4`.
- Node: `v25.4.0`; npm: `11.13.0`.
- Extension: `@mjakl/pi-processes@2.0.0` from npm.
- Extension peer requirement: `@earendil-works/pi-coding-agent >=0.84.2`.
- npm integrity:
  `sha512-LAt8fKvGuVManprlSsiJ0V6cDb5hlL9DlZrGpIW5dpgHVJKozgtBOiCKNovBZR4JYqlFIzbrBvJd9VEfvM0Gmg==`.
- Downloaded tarball SHA-256:
  `68377ce7fcf8465f754661a47f50581dea0a30c864fedb9c72c2820f99da14ee`.
- Locked isolated install recipe: `tests/fixtures/pi-process-wakeup/package-lock.json`.
- Neutral command fixture: `tests/fixtures/pi-process-wakeup/long-command.mjs`.

The adapter verifies the enclosing package name and exact version before spawn;
a missing, relative, or differently-versioned entry fails closed. The proof
used `/tmp/wg-pi-process-proof-package/.../src/index.ts`, installed with
`npm ci` from the fixture lock. It did not alter Pi settings, packages, auth, or
WG route configuration.

## Real provider proof

Using existing Pi-owned `openai-codex:gpt-5.6-sol` authentication, the candidate
WG binary launched real Pi in RPC mode. The model called the loaded `process`
tool once for:

```text
node tests/fixtures/pi-process-wakeup/long-command.mjs 2500 0 REAL_PROVIDER_WAKE_OK
```

It ended its model turn without `wait` or polling. The command outlived that
turn, exited 0, emitted one automatic `pi-processes:update`, and the model
continued in the same session and exact handle (`proc_1`). It called bounded
`process output` once and reported `REAL_PROVIDER_WAKE_OK`. The retained event
stream showed one managed `start`; no second command was launched. The adapter
record was bounded to command identity (512 Unicode scalars), PID, handle,
status, exit code and success; full extension logs remained file-backed and out
of the evidence/model context unless explicitly queried.

A second real-provider run started a 60-second managed command and an unrelated
90-second control process. SIGTERM of only the owning WG adapter caused Pi
session shutdown/pipe teardown and reaped the managed PID, while the unrelated
PID remained alive (`managed_alive=no`, `unrelated_alive=yes`).

## Race, duplicate and watchdog behavior

The adapter registers ownership only from the successful `process start` tool
receipt. Completion is reconciled by exact process handle and deduplicated. It
requires a `turn_end` after the completion message before exiting, which covers
a completion delivered before the model has visibly yielded without dropping
the continuation. Duplicate completion messages increment a diagnostic counter
but cannot launch another turn or command.

The Pi watchdog recognizes the real extension event shapes. A settled model with
an active managed handle is classified `LongTool` with
`managed_process_waiting_for_wake`; it is neither terminal nor fake progress.
The completion receipt is content-digested, updates bounded counters once, and
never stores command output, message content, or the raw handle.

Model-stream inactivity, command timeout, and the outer WG task deadline remain
separate. The exercised watchdog fixture advances its clock to 603 seconds—past
the normal 300-second model-idle suspicion bound—while the owned process is
active and proves no provider continuation or terminal action is emitted. The
adapter does not emit heartbeats and does not increase watchdog timeouts.

The real command was kept short (2.5 seconds); the shortened/advanced idle clock
is a controlled logical-time fixture rather than a five-minute wall-clock sleep.

## Persistence and authority boundaries

Supported persistence is intentionally only the **same live Pi RPC OS process
and Pi session**. `@mjakl/pi-processes` keeps its manager in memory. If that
process/daemon disappears, WG cannot prove ownership from a handle alone. A
resume attempt therefore fails with `WG-PI-PROCESS-REATTACH-UNPROVEN`, preserves
`pi-process-evidence.json` and the raw stream, and refuses blind re-execution.

Process events and log text are untrusted observations. They do not grant graph
scope and are not imported as an authoritative WG validation receipt. Success
and nonzero exit are recorded distinctly, but ordinary task success still runs
WG's configured host-side completion validation exactly once. This proof does
not rerun an expensive managed command merely to manufacture a gate receipt.

## Validation status and inherited baseline failures

The candidate-specific watchdog and adapter tests pass, as do `cargo fmt
--check`, `cargo clippy --locked`, the Worksgood Pi 29-test suite and host
selftest. Two configured integration targets have inherited failures on the
specified base commit `5a0b28a685962a63754fdb0e8563e512c00e0082` and on this
candidate:

- `integration_pi_sole_model_plane::missing_non_pi_and_missing_reasoning_fail_closed`
  panics at line 174 because `models.task_agent` is `None` before the test's
  `unwrap()`.
- `integration_service_control_permissions` has two worker cases refused as
  `worker_control.admin_operation_refused: command is outside trusted local
  graph coordination`, rather than the assertions' expected status/diagnostic.

Both were reproduced in a clean detached worktree at the exact base commit;
this task does not alter those tests or their control-plane code. The configured
combined gate therefore remains red for those pre-existing reasons, not because
the process-wake targets fail.

## Operator use

```bash
cd tests/fixtures/pi-process-wakeup
npm ci --ignore-scripts
export WG_PI_PROCESS_WAKE_EXTENSION="$PWD/node_modules/@mjakl/pi-processes/src/index.ts"
# Start the WG service/task normally with an explicit pi:<provider>:<model> route.
```

Only the extension's actual RPC contract is taught: `process start`, end the
turn, automatic wake, then bounded `process output`/`logs` if needed. There is
no `notify.logMatches` API in version 2.0.0, and no progress-message ceremony is
part of acceptance.
