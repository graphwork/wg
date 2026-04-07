# Surveillance Loop Audit: Pilot F Experiments

**Date:** 2026-04-07
**Scope:** pilot-f-5x1 (5 trials) and pilot-f-89 (90 trials)
**Question:** Were the surveillance loops in condition F genuinely functional or effectively no-ops?

## 1. What the Surveillance Task Actually Checks

The surveillance task is defined in `run_pilot_f_5x1.py:467–493` and identically in `run_pilot_f_89.py`. Each trial creates a surveillance agent with these explicit instructions:

1. **Run the verify command** — the exact same shell command used by the external harness (e.g., `test -f /tmp/project/src/main.py && ... && python3 -m pytest ...`)
2. **If exit code 0** → log "Verification passed — output is valid" → `wg done <surv-id> --converged`
3. **If non-zero** → log "Verification FAILED: ..." → `wg done <surv-id>` (without `--converged`, triggering cycle iteration)

The verify commands are substantive — they check file existence, JSON validity, test suite passage, output correctness, etc. They are not superficial existence checks.

**Key detail:** The surveillance agent receives `--exec-mode light` and `--context-scope graph`, meaning it has reduced tooling but full graph visibility.

### Source references
- Surveillance task description template: `run_pilot_f_5x1.py:467–493`
- Verify commands per task: `run_pilot_f_5x1.py:86–146` (5x1), `run_pilot_f_89.py:84–357` (89)

## 2. Cycle Configuration

| Parameter | Value | Source |
|-----------|-------|--------|
| Max iterations | 3 | `MAX_ITERATIONS` in both runner scripts |
| Cycle delay | 1 minute | `CYCLE_DELAY` in both runner scripts |
| Cycle structure | init → work → surv → work (back-edge) | Runner `wg edit` call at line 511 (5x1) |
| Cycle header | work task (not surv) | Ensured by init task as external predecessor |
| Failure restart budget | 3 | wg default for cycle_failure_restarts |

### The init-task fix (pilot-f-5x1 learning)

The original pilot-f-5x1 code discovered a cycle ordering bug: without an external predecessor, string-ID ordering made `surv-*` (s < w) the cycle header, causing surveillance to dispatch before work. The fix (lines 433–441) creates a one-shot `init-*` task that completes immediately, giving the work task an external predecessor and forcing it to become the cycle header.

**Evidence:** `run_pilot_f_5x1.py:433–441` comments explain the fix. The `run_pilot_f_89.py` docstring (line 14) confirms: "Init task as external predecessor ensures work (not surv) becomes cycle header."

After the fix, the dependency chain is: `init (done) → work → surv → work (back-edge)`. Work always runs before surveillance.

## 3. What "Converge First Try" Actually Means

### The metric definition (has a bug)

The summary's `trials_converged_first_try` is computed as:
```python
sum(1 for r in results
    if r["surveillance"]["converged"] and r["surveillance"]["iterations"] <= 1)
```

Where `converged` is derived by `extract_surveillance_stats()` (line 314–350):
```python
if status == "done":
    stats["converged"] = True
```

**This is misleading.** The code equates `surv.status == "done"` with "converged," regardless of whether the agent used `--converged`. A surveillance agent that does `wg done` (without `--converged`) — or one that makes zero tool calls and gets auto-completed by the wrapper — is counted as "converged first try."

### What actually happened (from graph data)

Examining the `graph.jsonl` files across all 90 trials in pilot-f-89:

| Surv task state | Count | Meaning |
|----------------|-------|---------|
| status=done + "converged" tag | 52 | Agent explicitly used `wg done --converged` |
| status=done, no "converged" tag | 6 | Agent used plain `wg done` or was auto-completed |
| status=failed | 32 | Work agent crashed (DNS errors), surv also failed |

The 32 failures correspond to the 29 DNS-failure trials that were re-run, plus 3 others. The summary's final numbers (86 converged first try) reflect the re-run results, but the on-disk graph states preserve the original (failed) runs.

### Corrected metric

Of the 58 trials where surveillance actually dispatched: **52 (89.7%) genuinely converged** (used `--converged`), 6 (10.3%) were marked done without explicit convergence.

## 4. Surveillance Agent Behavior: Genuine vs Rubber Stamp

By reading the `output.log` of each surveillance agent across all 90 pilot-f-89 trials:

| Classification | Count | Description |
|---------------|-------|-------------|
| Genuine verification | 55 | Agent used bash to run the verify command, logged result |
| Rubber stamp | 3 | Agent made 0 tool calls, output text only, wrapper auto-completed |
| Work/surv failed (DNS) | 32 | Never meaningfully dispatched |

**Genuine verification rate (of dispatched agents): 55/58 = 94.8%**

### Rubber stamp details

Three trials had surveillance agents that made zero tool calls:
- `f-algorithm-r1` (1 turn, 0 tools)
- `f-debugging-r0` (1 turn, 0 tools)
- `f-debugging-r1` (1 turn, 0 tools)

In these cases, the M2.7 model in `exec_mode=light` output text like "I'll verify the work..." but completed without invoking bash. The native executor wrapper then auto-marked the task as done: `[wrapper] Agent exited successfully, marking task done`.

### Genuine verification example

From `pilot-f-5x1/f-file-ops-r0`, agent-2 (surveillance):
```
Turn 2: bash → runs full verify command (test -f ... && pytest ...)
         → output: "1 passed in 0.06s"
Turn 3: bash → wg log surv-file-ops 'Verification passed — output is valid'
Turn 4: bash → wg done surv-file-ops --converged
```
Total: 4 turns, 13 seconds. Verification was real and substantive.

## 5. Could Surveillance Have Caught Failures?

### The 1 failed trial (f-iterative-test-fix-r1)

This trial's work agent crashed 4 times with exit code 1 (M2.7 couldn't fix 6 interrelated bugs in the scheduler). The cycle failure restart mechanism exhausted its 3-restart budget. Only then was the surveillance agent dispatched — and it also failed immediately (exit code 1 after 7 seconds).

**Timeline from graph.jsonl:**
```
07:39:22  init-iterative-test-fix → done (immediate)
07:39:22  work agent-1 spawned → 07:39:29 failed (exit 1)
07:39:29  Cycle failure restart 1/3
07:40:32  work agent-2 spawned → 07:42:51 failed (exit 1)
07:42:51  Cycle failure restart 2/3
07:43:53  work agent-3 spawned → 07:45:25 failed (exit 1)
07:45:25  Cycle failure restart 3/3
07:46:27  work agent-4 spawned → 07:47:47 failed (exit 1)
07:47:47  Cycle failure restart budget exhausted
07:47:47  surv agent-5 spawned → 07:47:54 failed (exit 1)
```

The failure was an agent crash, not incorrect output. Surveillance cannot help when the work agent itself cannot run.

### The 3 non-converged passing trials

- `f-mailman-r4`: Work agent crashed 4 times (DNS/agent errors), surv also failed. But the trial was re-run and passed in the re-run. The on-disk graph state is from the original failed attempt.
- `f-financial-document-processor-r3`, `f-financial-document-processor-r4`: Same pattern — original DNS failures, successful re-runs.

### Could surveillance have caught a "wrong answer" scenario?

**No, because it never happened.** Across all 90 trials:
- `issues_detected_count: 0`
- `total_iterations_across_trials: 0`
- `trials_needed_retry: 0`

Every work agent that successfully completed its task produced correct output on the first try. The surveillance loop's retry mechanism was never tested under conditions where work appeared done but verification failed.

The verify commands are substantive (running pytest, checking JSON validity, counting output rows, etc.), so if a work agent had produced incorrect output, a genuine surveillance agent *would* have caught it — the verification infrastructure was real. But M2.7 + graph context produced correct first-pass output 98.9% of the time (89/90), leaving nothing for surveillance to catch.

## 6. Assessment

### Verdict: Genuine but untested verification infrastructure

The surveillance loop was **not a rubber stamp** in the aggregate:
- 94.8% of dispatched surveillance agents genuinely ran the verify command
- The verify commands themselves were substantive (file checks, test suites, output validation)
- The cycle configuration (3 iterations, 1m delay, correct header ordering) was technically sound

However, surveillance was a **practical no-op**:
- It never found a problem to report
- It never triggered a cycle iteration
- The 5.2% rubber-stamp rate (3/58) shows the `exec_mode=light` + M2.7 combination is unreliable
- The `converged_first_try` metric in summary.json is inflated — it counts `status=done` rather than actual `--converged` usage

### Implications for the A vs F comparison

The A vs F result (both ~99% pass rate) is **primarily about context injection**, not surveillance catching errors:

1. **Context injection** (the wg Quick Guide + graph context scope) is what both conditions share and what likely drives the high pass rate
2. **Surveillance added overhead** (extra agent spawn, ~13 seconds per trial, ~2x agent count) without catching any errors
3. **The cycle mechanism was never stress-tested** — we don't know if it would work correctly in a scenario where work *looked* done but verification failed

### What this means for honest reporting

> "If surveillance was a rubber stamp, then the A vs F result is purely about context injection — which is actually a cleaner, simpler finding."

The evidence supports a nuanced middle ground: surveillance was *genuinely implemented* but *practically irrelevant*. The honest framing:

- M2.7 + context injection produces correct first-pass output at ~99% rate
- Surveillance infrastructure was technically functional but never activated
- The A vs F difference is about context injection + wg integration overhead, not error correction
- To properly evaluate surveillance value, we'd need tasks where first-pass failure rate is higher (harder tasks, weaker models, or adversarial conditions)
