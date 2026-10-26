# Fail-closed evaluation gate

## Incident and root cause

The durable reconciler formerly decided rejection with:

```text
evaluator_score < threshold && (FailedPendingEval || eval_gate_all || structural deliverables)
```

`wg done`, however, placed every ordinary task with a live evaluator in
`PendingEval`. A non-deliverable source was therefore displayed as gated while
the reconciler treated its score as advisory and promoted it to `Done`. FLIP was
linked but its score was not part of the source outcome. Successful execution of
`.flip-*` / `.evaluate-*` jobs could also display separate 1.00 evaluations,
which were easy to confuse with the source verdicts.

This explains both reported false passes:

- source attempt 1: FLIP 0.64, evaluator 0.18, effective threshold 0.70;
- replacement attempt 2: evaluator 0.12, again promoted;
- unrelated 1.00 scores belonged to evaluation-system job execution, not the
  source work.

## Contract

1. **`PendingEval` is always a hard gate.** An advisory evaluator never places
   its source in `PendingEval`; the source completes directly and records an
   `advisory` policy.
2. **Policy is attempt-pinned.** The source lifecycle snapshots applicability,
   evaluator threshold, FLIP policy, FLIP threshold, and threshold provenance.
   Config reloads affect future pipelines, not a gate already shown to a user.
   Bounded in-place rescue carries the same policy to the next exact source
   attempt.
3. **Required verdicts are independent.** The evaluator must meet its threshold.
   If `.evaluate-X` has persisted `.flip-X` as its dependency, FLIP is also
   required and must meet its own threshold. Scores are never averaged.
4. **Effective FLIP threshold.** An explicit
   `agency.flip_verification_threshold` is the hard-gate FLIP threshold;
   otherwise FLIP inherits `agency.eval_gate_threshold`. A hard-gated low FLIP
   does not create another verification satellite. The explicit setting keeps
   its legacy verification meaning only on advisory pipelines.
5. **Exact evidence only.** Verdict source task, pipeline id, source attempt,
   stage, durable evaluation digest, score, and stage/source kind must agree.
   Missing, duplicate, stale, mismatched, malformed, out-of-range, or non-finite
   evidence cannot promote. Evaluation-system self-evaluations are excluded by
   exact source identity and cannot substitute for a source verdict.
6. **Execution is not quality.** `Done` on `.flip-*` / `.evaluate-*` means the
   evaluation job executed. `wg show` says explicitly that this is not a source
   quality pass. The source's outcome provenance is authoritative.
7. **No manual scoreless bypass.** `wg approve` no longer accepts
   `PendingEval`; operators can wait for exact reconciliation or retry.

## Outcomes and replay

The evaluator verdict id remains the exactly-once source consumption fence; the
linked FLIP verdict id is recorded alongside it. Reconciliation stores a final
provenance summary naming each required verdict, score, threshold, and PASS/FAIL
result. Below-threshold `PendingEval` work either:

- reopens the same source task in place while under the configured rescue cap,
  with a new pipeline id/source-attempt and the same handler-first plans; or
- becomes terminal `Failed` after the bounded rescue budget is exhausted.

Downstream tasks remain blocked in both cases. Old verdict files are immutable
and remain available across restart/replay.

## Historical audit

Existing `Done` rows are never silently rewritten. If an old source has a
`consumed_verdict` but no persisted gate policy, `wg show <task>` performs a
deterministic exact-evidence audit against current effective thresholds and
labels it `historical-unclassified`. Missing/ambiguous evidence or any
below-threshold linked verdict emits `HISTORICAL AUDIT ALERT`; `wg status`
reports the number of such immutable outcomes. Operators can inspect the exact
source and choose an explicit retry without altering historical verdicts.

## Diagnostics

`wg status` shows:

- configured gate applicability;
- effective evaluator threshold;
- FLIP required/advisory policy and effective threshold;
- historical audit alert count.

`wg show <source>` shows the attempt-pinned policy, pipeline/source-attempt,
thresholds, outcome, exact verdict provenance, and audit result. `wg show` on an
evaluation system job distinguishes execution completion from source quality.

## Regression coverage

- `src/eval_lifecycle.rs` unit tests pin the 0.18/0.20 incident, exact-threshold
  pass, low-FLIP/high-evaluator and high-FLIP/low-evaluator failures,
  non-finite evidence, system-job 1.00 exclusion, two-attempt bounded rescue,
  stale attempt replay, exactly-once consumption, and immutable historical
  audit.
- `tests/integration_pending_eval_state.rs` pins hard vs advisory `wg done`
  structure.
- `tests/integration_global_config.rs` pins global/local effective threshold
  merge.
- `tests/smoke/scenarios/eval_gate_low_score_fail_closed.sh` runs the installed
  CLI and real durable writer/reconciler twice with credential-free Pi stubs,
  checks restart/tick behavior, downstream blocking, config reload/policy pin,
  system-job score exclusion, diagnostics, and advisory wording.
