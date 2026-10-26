# WG autopoietic loop failure triage

Date: 2026-06-03
Task: `triage-autopoietic-loop-failure`
Input report: `wg-autopoietic-loop-failure-report-20260603.md`

## Scope and evidence status

This triage used the report plus read-only inspection of the current WG graph,
task traces, source, and tests. No C4 scientific recovery tasks were resumed,
dispatched, edited, or marked terminal during triage. No API-key-bearing
environment dumps were inspected.

The historical C4 tasks named in the report are not present in this current WG
graph. `wg trace show autopoietic-c4-compression` and
`wg show c4-autopoietic-recovery` both return task-not-found, and
`wg list --all` has no entries matching `autopoietic-c4-compression`,
`c4-loop-0[1-4]`, `c4-autopoietic-recovery`, or
`c4-blocker-01-poasta-scale-impl`. Therefore, report statements about those
historical tasks are accepted as report evidence, while root-cause hypotheses
below are tied to locally inspected WG code paths.

Missing fact: the raw original `autopoietic-c4-compression` trace and graph rows
are unavailable in this workspace. If implementation needs exact historical
metadata, create a diagnostic task to import or recover the archived WG graph and
trace snapshot for the C4 incident.

## Failure mode 1: Codex optional image tool startup abort

Expected behavior: an unavailable optional Codex image/tool model must not abort
ordinary worker startup. WG should either omit/disable the optional tool or stop
before dispatch with a clear executor/tool preflight diagnostic that does not
consume structural-cycle failure restarts.

Actual behavior from report: `wg trace show --full autopoietic-c4-compression`
showed the Codex worker failing immediately with unavailable model
`gpt-image-2` under `param: tools`. The wrapper then marked the task failed and
the cycle consumed three failure restarts before exhausting.

Evidence from source:
- `src/commands/spawn/execution.rs:479` only preflights that the executor command
  exists. `preflight_executor_command` at `src/commands/spawn/execution.rs:1195`
  checks empty/missing binaries, not Codex tool/model compatibility.
- The Codex spawn arm at `src/commands/spawn/execution.rs:1474` builds
  `codex exec` from configured args and `--model`; there is no Codex-specific
  tool preflight or safe fallback.
- Built-in Codex settings in `src/service/executor.rs:1567` set the default
  command and args but do not isolate or validate optional Codex CLI tool config.
- `src/commands/codex_handler.rs:355` and `src/service/llm.rs:338` run
  `codex exec --json` directly for chat and lightweight roles. Nonzero Codex
  startup failures are surfaced after spawn, not classified before dispatch.
- The wrapper at `src/commands/spawn/execution.rs:1877` calls `wg fail` for a
  nonzero agent exit, which matches the report's wrapper failure and lets cycle
  restart handling treat this as a task failure.

Root-cause hypothesis: WG trusts the ambient Codex CLI configuration too late.
An invalid optional tool/model in Codex config reaches `codex exec`, fails before
the worker can read the task, and is then classified as a normal agent failure.

Suggested reproduction/regression path:
- Add a deterministic test using a temp `HOME`/`CODEX_HOME`, following the
  isolation pattern in `tests/codex_handler_oai_compat.rs`. The fixture should
  simulate or configure a Codex optional image tool referencing `gpt-image-2`.
- If the real Codex config key is unstable, use a fake `codex` shim on `PATH`
  that exits 1 with the historical stderr fragment. Assert WG reports an
  executor/tool preflight diagnostic before spawning task work, or at least
  classifies it as a configuration failure without burning cycle restart budget.
- Add a service-path smoke scenario under `tests/smoke/scenarios/`, owned by
  `fix-autopoietic-loop-failure`, using the fake Codex shim and temp config so it
  requires no network credentials or real API key.

Likely source/test areas:
- `src/commands/spawn/execution.rs`
- `src/service/executor.rs`
- `src/commands/codex_handler.rs`
- `src/service/llm.rs`
- `tests/codex_handler_oai_compat.rs`
- a new smoke scenario plus `tests/smoke/manifest.toml`

## Failure mode 2: blocked-open dependency cycle after root failure

Expected behavior: generated dynamic blocker work should not be left in a graph
shape where every useful child remains open but mutually blocked after the root
task fails. WG should warn, reject, or surface an actionable readiness diagnostic
for a cycle where no open member can become ready because all paths depend on
open peers and/or a failed root dependency.

Actual behavior from report: the root failed and generated `c4-loop-*` tasks
remained open. Examples from the report show `c4-loop-01` waiting on both the
failed root and `c4-loop-02`, `c4-loop-02` waiting on `c4-loop-01` and
`c4-loop-03`, and the rest participating in the same blocked cycle.

Evidence from source:
- `src/graph.rs:291` treats only `Done` and `Abandoned` as dependency-satisfying.
  A `Failed` root therefore correctly blocks downstream tasks.
- `src/query.rs:469` implements cycle-aware readiness by ignoring structural
  back-edges, but non-back-edge external dependencies remain blockers.
- `src/graph.rs:2547` and `src/graph.rs:2635` restart failed cycles by reopening
  members until the configured restart budget is exhausted. After exhaustion,
  failed dependencies still do not satisfy downstream children.
- Dispatcher maintenance invokes cycle failure restarts in
  `src/commands/service/coordinator.rs:4505`, then later dispatches only
  `ready_tasks_with_peers_cycle_aware` at `src/commands/service/coordinator.rs:4645`.
- Existing tests cover simple failed-upstream blocking, ordinary cycle readiness,
  cycle failure restarts, restart exhaustion, and open symmetric-cycle break-in.
  I did not find a regression matching the report's combined shape:
  failed root plus open children in a mutually blocked blocker cycle.

Root-cause hypothesis: WG has local cycle handling and local failed-dependency
semantics, but no diagnostic for "useful open tasks remain permanently
unready because a failed root is mixed into a blocker cycle." The cycle restart
budget can be spent on an executor startup failure, after which the graph is
legal but operationally dead.

Suggested reproduction/regression path:
- Add an integration fixture in `tests/integration_cycle_detection.rs` or
  `tests/integration_error_paths.rs`:

```text
root: Failed, cycle_config max_failure_restarts=3, cycle_failure_restarts=3
c4-loop-01: Open, after=[root, c4-loop-02]
c4-loop-02: Open, after=[c4-loop-01, c4-loop-03]
c4-loop-03: Open, after=[c4-loop-02, c4-loop-04]
c4-loop-04: Open, after=[c4-loop-03] or after=[c4-loop-03, c4-loop-01]
```

- Assert current readiness produces no useful child ready. The intended fix
  should assert a clear warning/rejection/recovery diagnostic from either
  graph validation, `wg ready`, or the service tick.
- Include a service-path smoke test if the fix changes dispatcher behavior or
  user-visible readiness diagnostics. The smoke should use a temp WG project and
  CLI graph construction, not any live C4 task state.

Likely source/test areas:
- `src/query.rs`
- `src/graph.rs`
- `src/commands/fail.rs`
- `src/commands/ready.rs`
- `src/commands/service/coordinator.rs`
- `tests/integration_cycle_detection.rs`
- `tests/integration_error_paths.rs`

## Failure mode 3: recovery task completed as evaluator-style documentation

Expected behavior: a supervisor/process-owner task whose validation requires a
loop, focused implementation subtasks, integration, reruns, and progress
artifacts should run those concrete actions or fail with a precise blocker. It
should not complete solely by documenting that no recovery artifacts exist.

Actual behavior from report: `c4-autopoietic-recovery` completed as an
evaluator-style report and its log said it found no recovery artifacts, runbook,
scoreboard script, C4 rerun, or successful subtask outcomes. That is evidence
that it evaluated the absence of loop outputs instead of supervising the loop.

Evidence from source:
- Historical task metadata and trace for `c4-autopoietic-recovery` are missing
  locally, so I cannot prove whether the task was routed as `bare`, `light`, or
  full, nor whether the selected agent simply chose a docs-only approach.
- Assignment prompt text in `src/commands/service/assignment.rs:197` describes
  `bare` as pure reasoning and `light` as read-only research/review/exploration.
  It does not name supervisor/process-owner tasks as requiring `full`.
- `src/commands/spawn/context.rs:1080` defaults task agents to `full` only when
  no task/role exec mode is set.
- `src/commands/spawn/execution.rs:888` skips worktree isolation for `bare` and
  `light`, which is correct for read-only modes but would be wrong for a
  supervisor required to create files or run implementation subtasks.
- The evaluator prompt in `src/agency/prompt.rs:400` reviews artifacts and logs
  and notes missing artifacts, but does not by itself enforce "process-owner
  tasks must create subtasks or progress artifacts" unless the task validation
  makes that explicit and evaluation catches it.

Root-cause hypothesis: this may be a routing/specification gap, not a single
confirmed code bug. The available source suggests the assigner may classify
"recover process" or "produce runbook" wording as read-only reasoning, and the
completion/evaluation path may not enforce a minimum progress artifact for
supervisor-loop tasks. Because the historical task is unavailable, treat this as
requiring either a separate diagnostic or a narrow implementation task that adds
explicit routing/evaluation invariants for supervisor/process-owner tasks.

Suggested reproduction/regression path:
- Construct a fixture task with tags such as `supervisor-loop` or
  `autopoietic-loop` and validation requiring at least one `wg add` child task,
  a scoreboard/progress artifact, and a rerun command or precise blocker.
- Assert assignment cannot choose `bare`/docs-only for this task, or assert the
  evaluation gate rejects completion without the required subtask/progress
  evidence.
- If implementation changes assigner or evaluator behavior, include focused unit
  coverage for prompt/routing plus an integration test that completes a
  supervisor-like task without artifacts/children and expects rejection.

Likely source/test areas:
- `src/commands/service/assignment.rs`
- `src/commands/spawn/context.rs`
- `src/commands/spawn/execution.rs`
- `src/commands/evaluate.rs`
- `src/agency/prompt.rs`
- assignment/evaluation integration tests

## Regression coverage recommendation

Split the fix if necessary. The Codex preflight/config problem and the
cycle/readiness diagnostic problem are independent and should each have a
failing regression first. The supervisor-vs-evaluator issue can be a third
follow-up if implementation cannot prove the historical routing path from
available evidence.

Required regression coverage:
- Codex invalid optional image/tool model preflight, with no real network or API
  credentials and no full environment logging.
- Blocked-open cycle graph fixture matching the report's failed-root plus
  mutually blocked child shape.
- Service-visible smoke coverage if the fix touches dispatcher spawn,
  readiness, cycle restart, or user-facing diagnostics.
- Supervisor/process-owner routing/evaluation coverage if that path is changed.

## Risks for implementation

- Service state: do not validate against the live daemon with real C4 tasks. Use
  temp WG projects and fake executors.
- Task graph mutation: graph validators must not silently rewrite user cycles or
  abandon tasks. Prefer warn/reject/diagnose unless a migration is explicit.
- Cycle restarts: executor configuration failures should not spend all useful
  loop retries, but changing restart semantics globally could mask real task
  failures. Classify narrowly.
- Secrets: tests and smokes must use temp `HOME`/`CODEX_HOME`, fake binaries, and
  synthetic keys only when needed. Do not print full process environments.
- Live daemon behavior: if the dispatcher surfaces the diagnostic, test the
  actual service path with a bounded smoke so the fix is not CLI-only.
