# WG autopoietic loop failure report

Date: 2026-06-03

## Summary

The C4 autopoietic compression loop did not fail because it evaluated the graph
and decided the scientific objective was blocked. It failed before doing useful
loop work because the WG execution path and dependency structure were wrong.

Two distinct process failures occurred:

1. The original structural-cycle task crashed at agent startup because Codex was
   launched with an invalid image tool model configuration.
2. The generated blocker tasks were wired into a cyclic dependency shape that
   deadlocked after the root task failed.

A later recovery task also did not run the loop. It was treated like an
evaluation/reporting task and produced a docs-only evaluation artifact instead
of creating/running implementation subtasks.

## Timeline

The root task was:

`autopoietic-c4-compression`

It was intended to:

- maintain a C4-vs-PGGB scoreboard,
- identify the largest remaining compression blocker,
- dispatch focused fix tasks,
- integrate successful fixes,
- rerun C4,
- repeat until convergence or a precise external blocker was proven.

The task did create four blocker task descriptions:

- `c4-loop-01-poasta-scale`
- `c4-loop-02-compound-scale`
- `c4-loop-03-seqwish-induction`
- `c4-loop-04-repeat-glue`

However, the root worker crashed before it could execute the process.

## Evidence

`wg trace show --full autopoietic-c4-compression` showed the worker failing
immediately with:

```text
The model 'gpt-image-2' does not exist.
param: tools
```

The wrapper then marked the task failed:

```text
[wrapper] Agent exited with code 1, marking task failed
```

WG retried the cycle three times:

```text
Cycle failure restart 1/3
Cycle failure restart 2/3
Cycle failure restart 3/3
Cycle failure restart budget exhausted
```

So the loop never reached its intended first evaluation/fix iteration.

## Dependency deadlock

The root task also created a structural cycle involving the generated blocker
tasks. After the root failed, the child tasks were left open but mutually
blocked. For example:

- `c4-loop-01-poasta-scale` waited on `autopoietic-c4-compression` and
  `c4-loop-02-compound-scale`.
- `c4-loop-02-compound-scale` waited on `c4-loop-01-poasta-scale` and
  `c4-loop-03-seqwish-induction`.
- `c4-loop-03-seqwish-induction` waited on `c4-loop-02-compound-scale` and
  `c4-loop-04-repeat-glue`.
- `c4-loop-04-repeat-glue` participated in the same blocked cycle.

This meant the useful blocker descriptions existed, but none could proceed.

## Recovery task failure mode

A follow-up task was created:

`c4-autopoietic-recovery`

It was meant to avoid structural cycles and run the blocker tasks sequentially.
Instead, the task completed as an evaluator-style report. Its own log stated:

```text
Evaluation evidence gathered: no task-scoped recovery artifacts, runbook,
scoreboard script, C4 rerun, or successful subtask outcomes found
```

That confirms the recovery task also did not perform the loop. It evaluated the
absence of loop artifacts rather than creating/running the implementation tasks.

## Root causes

### 1. Tool configuration bug

The primary startup failure was an invalid Codex tool configuration referencing
`gpt-image-2`. The model was not available, so the worker failed before it could
read or act on the task.

This should be treated as an executor/tooling configuration bug. A task should
not be able to fail at startup because an unrelated optional image tool points
to a nonexistent model.

### 2. Structural cycle was the wrong orchestration model

The C4 compression process needs iterative supervision, but the generated WG
structural cycle was brittle. Once the root task failed, the dependency graph
deadlocked the child tasks.

For this type of work, a safer shape is a sequential supervisor plus explicit
focused subtasks:

```text
scoreboard -> blocker-01 -> integrate/rerun -> blocker-02 -> integrate/rerun -> ...
```

The supervisor should create the next task only after the previous one produces
artifacts and a metric delta.

### 3. Task wording allowed evaluator behavior

The recovery task used language like "recover process" and "produce runbook",
which was interpreted as an evaluation/reporting task by the selected agent.
The task needed a concrete implementation command surface:

- create subtask X,
- wait for X,
- integrate result,
- rerun command Y,
- update report Z,
- create next subtask.

The new concrete task `c4-blocker-01-poasta-scale-impl` is the correct shape:
a specific implementation task with explicit evidence, invariants, validation,
and deliverables.

## Current state after recovery

The actually running replacement is:

`c4-blocker-01-poasta-scale-impl`

It is the first real blocker iteration:

- hypothesis: residual Poasta windows are selected at the wrong scale,
- scope: `src/resolution.rs` plus focused tests/report,
- validation: rerun C4 or representative C4 slice and compare metrics.

This is no longer a structural loop. Future iterations should be launched as
explicit next-blocker tasks based on the artifacts from the prior task.

## Recommendations for WG

1. Fix the Codex executor tool configuration so nonexistent optional tools do
   not abort worker startup. In this incident, `gpt-image-2` prevented all work.

2. Add a preflight executor check before dispatching an agent. If the tool/model
   configuration is invalid, fail the assignment task with a clear diagnostic
   rather than consuming cycle restarts.

3. Avoid structural cycles for complex research/implementation workflows where
   child tasks need to be generated and integrated dynamically. Use explicit
   sequential tasks or a supervisor that creates one blocker task at a time.

4. If WG creates a cycle, validate the dependency graph before publish. It
   should warn or reject cycles where every task waits on another open task and
   no task can become ready after a root failure.

5. Distinguish "supervisor/process owner" tasks from "evaluation/report" tasks.
   A task that says "run a loop and create implementation subtasks" should not
   be handled as a docs-only evaluator.

6. For long-running scientific loops, require a minimal progress artifact before
   completion:

   - scoreboard path,
   - at least one focused subtask ID and status,
   - latest metric delta,
   - next blocker.

## Practical next step

Continue from the concrete implementation task:

```bash
wg watch --task c4-blocker-01-poasta-scale-impl
```

When that task completes, use its report and metrics to create the next explicit
blocker task. Do not rely on the failed structural-cycle tasks.

