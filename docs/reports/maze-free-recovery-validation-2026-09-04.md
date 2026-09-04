# Maze-free recovery integration validation — 2026-09-04

Validated from `wg/agent-69/integrate-maze-free-recovery` with one freshly built release
candidate:

```text
/home/bot/.cache/wg/build-targets/2555b39f5ac57f2742255f1e49be26e3a7e797942ae41327825a896d186091cd/layers/638c932661b601e26bb96f668c5f5a9d20f67825cd9992aa2e79978ce01e80f9/agent-69/target/release/wg
sha256: 8117e169553ddc1c6f04812cefe9e4ea6d5182f61b8d3155f503e37d23fb642d
```

## Exact owned smoke outputs

Both commands used that exact path through `WG_SMOKE_CANDIDATE_BIN`.

### `service_start_readiness_pty`

Command:

```sh
WG_SMOKE_CANDIDATE_BIN=/home/bot/.cache/wg/build-targets/2555b39f5ac57f2742255f1e49be26e3a7e797942ae41327825a896d186091cd/layers/638c932661b601e26bb96f668c5f5a9d20f67825cd9992aa2e79978ce01e80f9/agent-69/target/release/wg bash tests/smoke/scenarios/service_start_readiness_pty.sh
```

The PTY emitted five readiness-confirmed starts (fresh plus four immediate stop/start
rounds), with PIDs `2656325`, `2656379`, `2656434`, `2656489`, and `2656543`. Its exact
terminal result line was:

```text
PASS: repeated PTY stop/start successes match exact ready daemons; failures are nonzero, loud on stderr, and JSON-safe
```

The failure subflow also asserted exact stderr markers `WG SERVICE START FAILED`,
`readiness timeout`, `Daemon log (last 20 lines)`, and
`Recovery: wg service start --force`; JSON asserted `status=failed` and
`recovery_command="wg service start --force"`.

### `completion_landing_reconciliation`

Command:

```sh
WG_SMOKE_CANDIDATE_BIN=/home/bot/.cache/wg/build-targets/2555b39f5ac57f2742255f1e49be26e3a7e797942ae41327825a896d186091cd/layers/638c932661b601e26bb96f668c5f5a9d20f67825cd9992aa2e79978ce01e80f9/agent-69/target/release/wg bash tests/smoke/scenarios/completion_landing_reconciliation.sh
```

Exact terminal output after the expected validation-command and no-running-service
warnings:

```text
Service stopped (PID 2667979), agents continue running
Service started and ready (PID 2668317)
Socket: /tmp/wgs-land-reconcile-2667883/adhoc.vonec9/project/.wg/service/daemon.sock
Log: /tmp/wgs-land-reconcile-2667883/adhoc.vonec9/project/.wg/service/daemon.log
Dispatcher: max_agents=1, poll_interval=1s, executor=pi, model=pi:openrouter:fake-worker
PASS: readiness-confirmed PTY stop/start preserved released-worker LandingPending state; WG runtime stayed administratively excluded; descendant target advance received renewed configured+baseline validation and an immutable target-binding receipt; supported resume landed the retained candidate without reset/retry/requeue/unclaim or Git history surgery
```

This composed flow asserted a released source worker, finalizer recovery authority,
unchanged candidate bytes, preserved target bytes, a clean attached checkout, exactly one
source invocation, exactly two semantic review calls (the strict review protocol, not a
post-restart rerun), two renewed validation envelopes, and a landing commit containing
both candidate and advanced-target parents.

## Other validation

- `cargo fmt` — pass.
- `cargo fmt --check` — pass.
- `cargo clippy` — pass with the repository's existing warnings.
- `cargo test --test integration_service` — pass: 9 passed, 0 failed, 3 intentionally
  ignored timing/legacy fixtures.
- `cargo test --test legacy_completion_authority_retired` — pass: 4 passed, 0 failed.
  This includes CLI help checks that only `merge-resolution status|inspect` are advertised
  and that landing recovery points to `wg resume <TASK> --only`, never a same-worker
  resubmission.
- `cargo test --bin wg commands::completion_land::tests -- --nocapture` and the analogous
  `commands::resume::tests` could not compile because seven pre-existing tests in unchanged
  `src/commands/profile_cmd.rs` call removed `parse_profile_use_target`. That file is
  byte-identical to `main`; this is a baseline test-only defect, not a candidate regression.
  The production binary, integration tests, and both real terminal flows compile and pass.
- `cmp AGENTS.md CLAUDE.md` — pass; neither guide changed and they remain identical.
- `cargo install --path . --locked` — pass; replaced global `wg`, `worksgood`, `nex`, and
  `casa-adapter`. Installed `/home/bot/.cargo/bin/wg` has the same SHA-256 as the validated
  candidate above.

## Supported recovery

1. Run `wg service stop --force`, then `wg service start --force`; success means the exact
   new nonce/PID/socket answered readiness.
2. For `Waiting/LandingPending`, run `wg show <task>` and
   `wg merge-resolution status <task>`.
3. Preserve user changes and clean the attached integration checkout.
4. Run `wg resume <task> --only`. Descendant target movement is integrated under renewed
   configured+baseline validation and an exact publication fence without the source worker.
5. For a fail-closed blocker, follow its exact status action. Use `wg reset <task>` only
   when status authorizes a new generation. Never retry/requeue/unclaim the retained
   candidate and never reset/rebase/cherry-pick its history.

## Changed files

- Operator/contributor docs: `README.md`, `docs/COMMANDS.md`,
  `docs/ops/maze-free-recovery.md`, this report, task-graph manual Markdown/Typst sources,
  and the worker-owned completion design/plan plus cleanup safety note.
- CLI/status/recovery: `src/cli.rs`, `src/main.rs`,
  `src/commands/{completion_land,quickstart,resume,show}.rs`.
- Regression coverage: `tests/legacy_completion_authority_retired.rs`,
  `tests/smoke/manifest.toml`, and both owned smoke scripts.

## Residual risks

- Divergence and semantic merge conflicts intentionally do not auto-resolve. They retain
  bytes and evidence and may require the status-authorized new-generation path.
- PTY/service coverage here is Linux/Unix-socket based; other platform service adapters
  retain their own environmental risk.
- Fake Pi makes the composed smoke deterministic and credential-free; it proves lifecycle,
  receipt, fence, worktree, and invocation-count behavior, not live provider quality.
- Historical audit documents still quote the old same-worker FLIP rejection text as dated
  evidence. Current help, errors, manuals, design guidance, and quickstart no longer use it
  as landing-recovery advice.
