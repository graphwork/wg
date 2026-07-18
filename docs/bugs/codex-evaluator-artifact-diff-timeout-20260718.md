# Codex evaluator omits artifact diff and times out in repository inspection

Date: 2026-07-18

## Summary

`wg evaluate run` computes a bounded artifact diff and passes it through
`EvaluatorInput.artifact_diff`, but `render_evaluator_prompt` did not render
that field. A Codex-backed evaluator therefore received artifact paths without
the corresponding code evidence. It attempted repository inspection and test
execution through nested tools, repeatedly reached the evaluator's 300-second
hard timeout, and left the source task in `pending-eval`.

This was reproduced on Frontier while evaluating Emender task
`integrate-resilient-pool-v1` at commit
`ae2e6f26046fb7a6b348e845fb4615092a7c37e0` with route
`codex:gpt-5.6-luna`.

## Impact

- The implementation had already completed, merged, and pushed.
- FLIP completed with score 0.82.
- The exact-tree validation suite passed 120 tests in 98.59 seconds.
- The ordinary evaluator timed out 26 times and kept the parent task in
  `pending-eval`.
- The one-minute park/retry policy turned a deterministic evaluator failure
  into an unbounded retry loop.

No training or Slurm job failed in this incident. The blockage was entirely in
WG's semantic evaluation path.

## Evidence

The evaluator process tree contained a literal timeout wrapper:

```text
wg evaluate run integrate-resilient-pool-v1
  /usr/bin/timeout 300s codex exec --json ... --model gpt-5.6-luna
```

Attempts either exhausted that deadline with exit code 124 or reported nested
tool failures such as:

```text
Failed to create unified exec process: No such file or directory (os error 2)
write_stdin failed: Unknown process id
```

One attempt spent most of its budget rerunning the already-recorded 120-test
pytest command. The evaluator output log never advanced beyond the initial
evaluation message before the wrapper terminated it.

## Root cause

`src/commands/evaluate.rs` correctly calls `compute_artifact_diff`, caps the
result at `MAX_DIFF_BYTES`, and assigns it to `EvaluatorInput.artifact_diff`.
Before this fix, `src/agency/prompt.rs::render_evaluator_prompt` rendered the
artifact path list and then advanced directly to the task log. The diff was
never included.

The lightweight LLM call launches Codex with normal tool access. With no code
content in the prompt, repository inspection is a reasonable model response,
but it defeats the intended bounded one-shot evaluator behavior.

## Contributing behavior

The evaluated task was pinned to named profile `codex`. Its effective profile
configuration did not carry the project's `agency.triage_timeout = 900`, so the
evaluation used the code's 300-second minimum. Increasing that timeout would
only hide the missing-evidence bug and permit more duplicate test work.

After exit 124, WG classified the failure as an execution-route failure, parked
the evaluator for one minute, and dispatched the same request again. A
deterministic wrapper timeout should receive a bounded retry budget or require a
changed route/payload before retrying.

## Fix in this branch

1. Render `artifact_diff`, when present, in a fenced `## Artifact Diff` section.
2. State that the evaluator prompt is self-contained and must not invoke tools,
   inspect the repository, or rerun verification commands.
3. Extend the evaluator prompt regression test to require both the diff and the
   no-tools instruction.

The existing 30,000-byte cap remains the prompt-size bound.

## Validation

- `cargo fmt --check`: passed.
- Focused test was added to `test_render_evaluator_prompt_full`.
- On Frontier, the initial focused test build was blocked by the active Cray
  compiler wrapper injecting unsupported LTO plugin flags into `rust-lld`.
- Retrying with `/usr/bin/gcc` progressed through compilation. Adding
  `LIBCLANG_PATH=/opt/cray/pe/cce/18.0.1/cce-clang/x86_64/lib` advanced
  `boring-sys2`/bindgen further, but that Cray libclang could not find the host
  `stddef.h`. The focused test therefore did not reach its test body on this
  Frontier login environment.

Recommended validation command on a normal host or CI runner:

```bash
cargo fmt --check
cargo test test_render_evaluator_prompt --lib
cargo clippy
```

## Follow-up recommendations

- Add a bounded retry budget for identical evaluator exit-124 failures.
- Decide whether operational timeouts should be inherited from project config
  when a named profile supplies routing.
- Consider enforcing no-tool execution at the Codex invocation layer, rather
  than relying solely on the prompt, for all lightweight agency calls.
