# Merge Clean Luca PRs Audit - 2026-07-11

Task: `merge-clean-luca`

Repository: `graphwork/wg`

Scope: sequentially gate and land Luca Pinello PRs #52, #53, then #50.
GitHub merge/comment actions were authorized by the task. Source files were not
edited outside the normal PR merge flow.

## Results

- #52 `fix(platform_timeout): probe gtimeout/timeout, watchdog fallback on macOS without coreutils`
  - PR: https://github.com/graphwork/wg/pull/52
  - Head SHA: `85281b4939d746c96785d220cabbfd19267020ea`
  - GitHub gate before merge: open, non-draft, base `main`,
    `CLEAN`/`MERGEABLE`, all five CI checks successful.
  - Local validation on exact head: `cargo test platform_timeout` passed
    (5 matching tests, 0 failed).
  - Merge strategy: GitHub merge commit, preserving contributor commits.
  - Merged at: `2026-07-11T15:30:54Z`
  - Merge commit: `51db806b53e2ef3b857a695f097e5b3967dd66b7`

- #53 `test: fix flaky session_lock PID-reuse tests (transient comm during fork/exec window)`
  - PR: https://github.com/graphwork/wg/pull/53
  - Head SHA: `040325282048bc13fa3b31833933237d329a611b`
  - Re-evaluated only after #52 was merged and `origin/main` refreshed to
    `51db806b53e2ef3b857a695f097e5b3967dd66b7`.
  - GitHub gate before merge: open, non-draft, base `main`,
    `CLEAN`/`MERGEABLE`, all five CI checks successful.
  - Local validation on exact head: `cargo test session_lock` passed
    (20 matching library tests plus the related coordinator classification
    test, 0 failed).
  - Merge strategy: GitHub merge commit, preserving contributor commits.
  - Merged at: `2026-07-11T15:34:43Z`
  - Merge commit: `e6f46509e4863690714efe5d4d8bb7acfc892c8e`

- #50 `feat: bind named-agent identity to a persistent session (R2)`
  - PR: https://github.com/graphwork/wg/pull/50
  - Head SHA: `842cdf94038d13141f8c3c167aef75d9094bbafd`
  - Re-evaluated only after #53 was merged and `origin/main` refreshed to
    `e6f46509e4863690714efe5d4d8bb7acfc892c8e`.
  - GitHub state at refresh still reported open, non-draft, base `main`,
    `CLEAN`/`MERGEABLE`, and the prior CI checks successful.
  - Local validation on exact head failed: `cargo test bind_agent` did not
    compile because existing `TemplateVars` struct initializers were not updated
    for the new `bound_session_summary` field.
  - Representative failure: `error[E0063]: missing field 'bound_session_summary'
    in initializer of 'TemplateVars'`.
  - Affected locations reported by rustc:
    - `tests/integration_context_scope.rs`
    - `src/commands/spawn/execution.rs`
  - Prompt-size/stale-memory review: the PR injects only the existing
    `session-summary.md` summary, not full conversation history; injection is
    skipped when unbound or empty and is labelled as continuity rather than
    current-task instructions. No separate stale-memory blocker was found, but
    the compile failure blocks merge.
  - Action taken: #50 was not merged. Posted maintainer comment:
    https://github.com/graphwork/wg/pull/50#issuecomment-4947166652

## Validation Outcome

- #52 macOS timeout fallback: green and merged safely.
- #53 PID-reuse test stabilization: green and merged safely after #52.
- #50 persistent-session change: not merged; exact-head local validation failed.
- Because #50 failed a gate, this task was not marked done and should remain
  parked until #50 is revised and revalidated.
