# Smoke Test: Condition F Without Surveillance

**Date:** 2026-04-07T19:53:05.084774+00:00
**Model:** openrouter:minimax/minimax-m2.7
**Runner:** Stripped (no surveillance loops)
**Replicas:** 1
**Tasks:** 4 (1 easy, 1 medium, 2 hard)
**Total trials:** 4
**Wall clock:** 126s (0.0h)

## Purpose

Verify that condition F still works after removing surveillance loops
from the runner (tb-strip-surveillance). Confirm pass/fail consistency
with pilot-f-89 for the same tasks.

## Overall Results

| Metric | Smoke (no surv) | pilot-f-89 (same tasks) |
|--------|-----------------|------------------------|
| pass_rate | 100.0% (4/4) | 100.0% (20/20) |
| mean_time_s | 53.3s | 136.6s |
| tokens/trial | 107,619 | ~143,000 |

## Per-Task Comparison: Smoke vs pilot-f-89

| Task | Difficulty | Smoke Pass | Smoke Time | pilot-f-89 Pass Rate | pilot-f-89 Mean Time |
|------|-----------|------------|------------|----------------------|----------------------|
| file-ops | easy | 1/1 (100%) | 42.0s | 5/5 (100%) | 199.4s |
| debugging | medium | 1/1 (100%) | 36.9s | 5/5 (100%) | 79.3s |
| algorithm | hard | 1/1 (100%) | 46.8s | 5/5 (100%) | 142.8s |
| build-cython-ext | hard | 1/1 (100%) | 87.5s | 5/5 (100%) | 124.3s |

## Analysis

- **All 4 tasks pass** — matching pilot-f-89 results exactly.
- **No regressions** from removing surveillance infrastructure.
- **Timing is faster** across the board. This is expected: pilot-f-89 included
  surveillance loop setup overhead (cycle creation, convergence checks) even
  though 0 surveillance iterations actually fired across all 90 trials.
- **Surveillance was confirmed to be dead weight** — 0 iterations across all
  pilot-f-89 trials, and removing it changes nothing about pass/fail behavior.
- **wg context injection confirmed intact**: all trials used graph context scope,
  WG Quick Guide, and wg CLI (verified via config.toml in each trial's workgraph_state).

## Conclusion

The stripped runner (no surveillance loops) produces identical pass/fail
results to the original runner for these 4 representative tasks. Safe to
proceed with the full-scale experiment using the stripped runner.
