# Evaluation retry pipeline drift repair

Task: `fix-eval-retry-pipeline-drift`  
Incident: 2026-07-24 `fix-stale-split` PendingEval stall

## Root cause

A source retry could advance the parent's derived evaluation attempt without atomically rebinding the already-scaffolded `.flip-*` and `.evaluate-*` rows. The source then waited on attempt 2 while both terminal satellites and their immutable durable verdicts still named attempt 1. Exact-match reconciliation correctly refused those old verdicts, but did not repair or diagnose the stranded pipeline.

The preemption path also exposed a race: `wg retry` waits for a graceful worker kill outside the graph lock, allowing dead-agent reconciliation to open the same retry generation first. Treating the later retry mutation as a second attempt produced attempt 3 from one preemption.

## Implemented invariants

- `begin_source_attempt` is the authoritative atomic graph mutation for a new implementation attempt. It mints one parent pipeline and rearms every existing evaluation satellite to that exact pipeline and source attempt.
- Rearm clones the validated persisted `AgencyCallPlan`; handler-first route, endpoint, reasoning, execution system, fallbacks, and stage ordering are retained byte-for-byte. Ambient configuration is never consulted.
- Retry, incomplete retry, explicit/operator preemption, dead-agent cleanup, orphan sweep, zero-output reset, resource retry, requeue, and auto-rescue invoke attempt minting in the same graph transaction as the source reset.
- The graceful-kill race recognizes an already-open, unclaimed generation and does not increment or mint twice.
- Durable verdict files remain immutable. Reconciliation still requires exact source task, pipeline, source attempt, and stage. Stale verdicts remain historical evidence and cannot score a later attempt.
- PendingEval repair is bounded to one atomic plumbing repair per source attempt. It rearms only unambiguous, unconsumed work from persisted routes. Active mismatched runs, ambiguous routes/verdicts, or exhausted repair budgets produce stable `WG-EVAL-*` operator diagnostics and fail closed.
- Repeated reconciliation is idempotent: durable evidence is linked once and an evaluator verdict is consumed once.
- `wg status`, `wg show`, and `wg why-blocked` report `active-evaluation`, `repairable-pipeline-drift`, or `operator-required-ambiguity`. PendingEval is no longer counted as implementation-worker progress.

## Regression coverage

Deterministic tests cover:

- attempt-1 preemption followed by attempt-2 mint/rearm;
- failed retry, incomplete retry, resume-in-place, and fresh retry;
- exact route/reasoning preservation;
- stale and duplicate durable verdicts;
- FLIP-only evidence plus evaluator failure;
- bounded repair and stable ambiguity diagnostics;
- historical claimed rows that must never be relabeled;
- daemon-style graph round trips and exactly-once consumption;
- the graceful-kill/dead-agent reconciliation race.

The registered `eval_retry_pipeline_drift` smoke uses a real WG service with a credential-free fake Pi process. It preempts live source attempt 1, verifies the atomic attempt-2 parent/satellite state before evaluation dispatch, restarts the daemon around retry and source completion, runs the attempt-2 FLIP/evaluator chain, verifies one consumption, and observes downstream dispatch.

## Validation

See `docs/reports/eval-retry-pipeline-drift-validation.log` for the command/result record.
