# WG Executor/Model Mismatch PR CI

Task: `pr-ci-wg-executor-model-mismatch`

## PR

- PR: https://github.com/graphwork/wg/pull/42
- Base branch: `main`
- Head branch: `wg/agent-1692/fix-wg-executor-model-mismatch`
- Checked head SHA: `55dbbe4b205015a48d711eca0c39b76fd1ef8cac`
- Workflow run: https://github.com/graphwork/wg/actions/runs/27023824284

## Remote Branch

`origin/wg/agent-1692/fix-wg-executor-model-mismatch` was pushed and verified
with `git ls-remote`; the remote head matched local commit
`55dbbe4b205015a48d711eca0c39b76fd1ef8cac`.

## Check Results

Observed on 2026-06-05.

| Check | Conclusion | Evidence |
| --- | --- | --- |
| `Build & Test (stable)` | success | `cargo build`, unit tests, and doc tests completed successfully |
| `Build & Test (nightly)` | success | `cargo build` and `cargo test` completed successfully |
| `Check & Lint` | failure | `cargo fmt --check` failed with a diff in `tests/integration_cli_workflows.rs` |
| `Integration Tests (stable)` | failure | `cargo install (needed by integration tests)` failed because crates.io had yanked all matching `rquest = "^3.0.0"` versions used without `--locked` |

## Outcome

Remote CI reached a terminal state with overall conclusion `failure`. The PR
must not be integrated from this validation run. A targeted follow-up should fix
or route around the CI failures before integration proceeds.
