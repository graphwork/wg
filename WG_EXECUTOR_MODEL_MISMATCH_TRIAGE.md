# Triage: executor/model mismatch

Task: `triage-wg-executor-model-mismatch`

## Confirmed Scenario

The report targets a backend consistency failure in lightweight assignment and
dispatch. The failure chain is:

- lightweight assignment picks an agent and task-agent model for real work;
- the selected model is codex-class, observed as `gpt-5.5`;
- the spawn path keeps or resolves the executor as `claude`;
- the task log shows a doomed pair: `--executor claude --model gpt-5.5`;
- the agent exits within roughly five seconds before doing work or calling
  `wg done`, producing generic `agent-exit-nonzero` / failed-pending-eval
  rescue failure rather than a specific routing error.

The local graph no longer contains the two historical task IDs named in the
report (`causal-test-flip`, `texaudit-s8-fill`), so those records could not be
independently replayed from this checkout. The report and upstream quality
artifact are internally consistent, and the current task itself demonstrates
the expected compatible route: runtime `executor=codex`, configured
`codex:gpt-5.5`.

## Likely Root Cause

The highest-risk path is split model/provider handling:

- `src/commands/service/coordinator.rs` resolves the default task-agent model
  with `config.resolve_model_for_role(DispatchRole::TaskAgent).model`.
- `src/config.rs::resolve_model_for_role` can return a registry/API model id
  such as `gpt-5.5` while carrying provider separately in
  `ResolvedModel.provider`.
- `src/dispatch/plan.rs::plan_spawn` receives only the model string. Bare model
  strings are treated as Claude-compatible aliases, so `gpt-5.5` without a
  `codex:` provider prefix can leave `executor=claude`.
- `src/commands/spawn/execution.rs` logs the actual spawn pair as
  `Spawned by coordinator --executor <executor> --model <effective_model>`.

Existing dispatch tests cover several compatibility cases, including
`local:`/native and explicit codex executor floors, but the exact regression
fixture should cover task-agent model resolution returning or preserving
`codex:gpt-5.5` instead of bare `gpt-5.5`.

## Recommended Reproduction

Add a focused regression around the spawn planning path used by the dispatcher:

1. Configure task-agent routing/tier resolution so the selected worker model is
   codex-class (`codex:gpt-5.5` or a registry alias that previously collapsed
   to bare `gpt-5.5`).
2. Use a default/non-explicit agency agent so the agent executor does not force
   codex itself.
3. Plan or smoke a ready user task after lightweight assignment.
4. Current failing behavior to pin: `SpawnPlan` or task log resolves
   `executor=claude` with `model=gpt-5.5`.
5. Expected behavior: `codex:gpt-5.5` routes to executor `codex`; `claude:opus`
   routes to executor `claude`; incompatible pairs are rejected before launch
   with a specific executor/model backend mismatch.

A service smoke fixture should be modeled after the existing routing scenarios
in `tests/smoke/scenarios/`, especially:

- `codex_cli_fresh_init_runtime.sh`
- `agency_codex_override_routes_to_codex.sh`
- `dispatcher_codex_wins_over_agency.sh`
- `evolve_fanout_codex_route_no_bare_aliases.sh`

The new smoke should use a scratch WG project, avoid real LLM calls where
possible, start the dispatcher long enough to capture the `SpawnPlan`/spawn log,
and assert that a codex task-agent route never logs `executor=claude` with a
codex-class model.

## Source And Test Areas

Likely source areas:

- `src/config.rs`: `ResolvedModel` / `resolve_model_for_role` provider-prefixed
  model preservation, tier and registry behavior.
- `src/commands/service/coordinator.rs`: lightweight assignment mutation and
  `spawn_agents_for_ready_tasks` default task-agent model handoff into
  `plan_spawn`.
- `src/dispatch/plan.rs`: model/executor compatibility and pre-spawn rejection
  or override semantics.
- `src/dispatch/handler_for_model.rs`: expected mapping (`codex:*` -> codex,
  `claude:*` and bare Claude aliases -> claude).
- `src/commands/spawn/execution.rs` and `src/commands/spawn_task.rs`: final
  spawn logging/env/metadata should reflect the same plan.

Likely tests:

- `src/dispatch/plan.rs` unit tests for exact `codex:gpt-5.5` and bare
  `gpt-5.5` mismatch behavior.
- `src/config.rs` tests for `resolve_model_for_role(TaskAgent)` preserving a
  provider-qualified codex route where spawn planning needs it.
- `tests/integration_auto_assignment.rs` and/or service coordinator tests for
  assignment mutation plus dispatcher handoff.
- A permanent smoke scenario under `tests/smoke/scenarios/` with an owner entry
  in `tests/smoke/manifest.toml` if the fix changes service-visible dispatch.

## Gates And Risks

Local gates for implementation:

- focused unit/integration tests for touched files;
- `cargo build`;
- `cargo test`;
- `wg config lint`;
- focused smoke for codex task-agent dispatch, plus any owned smoke scenarios.

Remote PR gates:

- pushed branch SHA matches local fix SHA;
- PR targets the correct main branch;
- all required remote PR checks complete and pass;
- record exact check names, conclusions, checked SHA, and PR URL before
  integration.

Risks:

- Do not print or commit API keys, keyring values, endpoint auth, or
  secret-bearing env output.
- Avoid depending on a live daemon's current profile while reproducing; isolate
  `HOME`/`XDG_CONFIG_HOME` in smoke tests.
- `wg config lint` currently reports project-local config clean but global
  config has an unrelated stale `tiers.standard` warning
  (`codex:gpt-5.4` -> `codex:gpt-5.5`).
