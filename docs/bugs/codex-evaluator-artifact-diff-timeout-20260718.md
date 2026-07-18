# Codex evaluator omitted artifact-diff evidence and exhausted its deadline

Date: 2026-07-18

## Summary

`wg evaluate run` computed artifact-scoped Git evidence and assigned it to
`EvaluatorInput.artifact_diff`, but `render_evaluator_prompt` did not render that
field. The evaluator therefore received artifact paths without the corresponding
code evidence. On the reported Frontier run, a Codex-backed evaluator attempted
repository inspection and verification work and repeatedly exhausted the
one-shot evaluator deadline.

The code-level evidence omission is reproduced locally by the permanent
`evaluator_artifact_diff_prompt` smoke scenario. The Frontier process/output
logs were not included in the submitted branch, so the incident counts and
remote command excerpts below remain explicitly reporter-supplied rather than
independently reproduced facts.

## Reported incident

The original report identified Emender task `integrate-resilient-pool-v1`, tree
commit `ae2e6f26046fb7a6b348e845fb4615092a7c37e0`, and evaluator route
`codex:gpt-5.6-luna`. It also reported that:

- the implementation had completed, merged, and pushed;
- FLIP scored the work at 0.82;
- an exact-tree suite passed 120 tests in 98.59 seconds;
- 26 ordinary-evaluator attempts timed out while the source task remained
  `pending-eval`;
- no training or Slurm job failed.

Those operational values are useful incident context, but no matching logs are
present in this repository or the submitted commit. They should not be read as
claims verified during this review.

The reporter supplied this process shape for the Linux host:

```text
wg evaluate run integrate-resilient-pool-v1
  /usr/bin/timeout 300s codex exec --json ... --model gpt-5.6-luna
```

That shape is consistent with the reviewed source: `call_codex_cli` delegates to
`platform_timeout::spawn_with_timeout`, which uses GNU `timeout` when available
on Unix and otherwise uses the watchdog implementation. The exact `/usr/bin`
path and observed exit 124 are not reproduced here. The reporter also supplied
nested-tool errors including:

```text
Failed to create unified exec process: No such file or directory (os error 2)
write_stdin failed: Unknown process id
```

## Confirmed root cause

At base commit `604076593e9d6fb8e2dfd0529ac2f83806063c9f`:

1. `src/commands/evaluate.rs::compute_artifact_diff` ran `git diff` over the
   task's recorded artifact paths and bounded the result.
2. `commands::evaluate::run` assigned that value to
   `EvaluatorInput.artifact_diff`.
3. `src/agency/prompt.rs::render_evaluator_prompt` rendered the artifact path
   list and then advanced directly to `## Task Log`; it never read
   `artifact_diff`.

The production-path regression creates a real Git-backed task, records an
artifact, and runs:

```bash
wg evaluate run eval-diff \
  --evaluator-model codex:gpt-5.6-luna \
  --dry-run
```

It fails on the base commit because the actual dry-run evaluator prompt has no
artifact-diff section. This proves the evidence plumbing defect without
requiring Frontier or Codex credentials; it does **not** claim to reproduce the
remote 300-second timing behavior.

## Timeout and retry behavior

`commands::evaluate::run` computes the evaluator deadline as
`max(agency.triage_timeout.unwrap_or(60), 300)`. The submitted report attributed
the observed 300 seconds to a named-profile merge dropping a project value of
900 seconds. That configuration assertion could not be checked without the
incident's effective config and has been removed as a confirmed cause.

A lightweight-call failure is returned immediately by `evaluate::run`; its
three-attempt loop retries JSON extraction failures, not a failed Codex process.
The agency satellite wrapper separately treats an execution-route failure as
retryable and parks it for one minute. The reviewed wrapper does not attach an
identical-payload timeout budget at that point, so repeated re-dispatch is a
credible follow-up risk. It is not changed in this narrowly scoped fix.

## Reviewed fix

The reviewed implementation:

1. renders the artifact diff in the production evaluator prompt;
2. enforces the 30,000-byte evidence budget both when Git output is collected
   and defensively in the renderer, with the truncation notice inside that
   budget;
3. wraps the diff in a collision-free, explicitly untrusted-data boundary
   rather than an escapable fixed Markdown fence;
4. tells the one-shot evaluator to use only supplied evidence and not invoke
   tools, inspect the repository, or rerun verification commands;
5. covers the renderer with unit/snapshot tests and the full CLI prompt path
   with a credential-free smoke scenario.

The cap applies to the artifact-diff evidence, not to every other evaluator
field (task description, identity, and log). No broader whole-prompt-size claim
is made.

The change is provider-agnostic: Codex, Pi, Claude, and native/Nex evaluators use
the same prompt renderer and receive the same bounded evidence. It does not
alter route resolution, model identity, timeouts, execution-system policy, or
fallback configuration. In particular, this change does not add an implicit
fallback.

## Tool-access boundary

The no-tools rule is currently a prompt contract for Codex and Claude CLI
one-shots. The Codex invocation still uses
`--dangerously-bypass-approvals-and-sandbox`; this patch does not claim to
remove Codex tool capability at the process layer. Pi one-shots already pass
`--no-tools`, while native API calls send an empty tool list. Enforcing a
Codex-specific no-tool execution mode, if the CLI exposes a stable mechanism,
should be a separate reviewed change rather than a provider-specific branch in
the evaluator prompt.

## Candidate provenance and review

The named remote branch was fetched at exactly
`07bdec727dce5bdcb23a12906d7aaaa5ac800cc1` (tree
`d55d407eb718bdad532ab17dd221b0f5e0653a23`, parent
`604076593e9d6fb8e2dfd0529ac2f83806063c9f`). The author and committer are Erik
Garrison `<erik.garrison@gmail.com>`; the commit is unsigned. `FETCH_HEAD` and
`origin/fix/codex-evaluator-artifact-diff-timeout` both matched the requested
commit, so no branch movement was used.

The submitted commit changed only `src/agency/prompt.rs` and this report. Review
found two issues before landing: the existing evaluator snapshot was not
updated, and the fixed triple-backtick diff fence could be closed by untrusted
artifact content. Both were corrected in the reviewed integration.

## Validation

Validated in an isolated worktree and isolated `CARGO_TARGET_DIR`:

- base-commit production regression: fails with missing artifact-diff section;
- focused evaluator prompt unit tests;
- evaluator prompt snapshots;
- production `evaluator_artifact_diff_prompt` smoke, including route identity,
  a 30KB cap, UTF-8-safe truncation, and delimiter-collision payload;
- evaluator/agency route tests confirming explicit handler identity and no
  hard-coded fallback;
- `cargo fmt --check`, `cargo clippy`, `cargo build`, and clean-environment full
  `cargo test` (see task log for exact commands and final results).

## Follow-up recommendations

- Add a bounded retry policy keyed by an unchanged evaluator payload/route after
  repeated deadline failures.
- Surface the effective evaluator timeout and config source in dry-run output.
- Investigate stable process-level no-tool enforcement for Codex one-shots.
