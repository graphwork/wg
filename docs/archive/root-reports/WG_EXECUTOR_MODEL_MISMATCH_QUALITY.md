# Quality pass: executor/model mismatch batch

Task: `quality-wg-executor-model-mismatch`

Reviewed report: `WG_EXECUTOR_MODEL_MISMATCH.md`

## Findings

- The report describes a backend consistency bug in lightweight assignment:
  a codex-class model such as `gpt-5.5` can be written while the executor
  remains `claude`, producing a doomed spawn like
  `--executor claude --model gpt-5.5`.
- The downstream chain is sequential and matches the report:
  `triage-wg-executor-model-mismatch` ->
  `fix-wg-executor-model-mismatch` ->
  `pr-ci-wg-executor-model-mismatch` ->
  `integrate-wg-executor-model-mismatch`.
- All downstream tasks are pinned to `codex:gpt-5.5` and now use graph
  context so workers receive the report and predecessor handoff.
- The fix task now explicitly requires a failing regression test before the
  fix, with coverage for both atomic assignment and pre-spawn mismatch
  rejection.
- The PR/CI task now explicitly requires pushing the fix branch to origin,
  opening or updating the PR, and observing remote PR checks to terminal
  passing status.
- The integration task now explicitly waits on PR/CI success and requires the
  merge path to be documented.

## Scope

This pass intentionally made no Rust/source implementation changes.
