# TB Pilot: Condition A' (Native WG Adapter)

**Date:** 2026-04-05
**Model:** openrouter:minimax/minimax-m2.7
**Trials:** 14
**Executor:** native WG (per-trial service instances)
**Federation:** tb-evaluations/ hub

---

## Summary

| Metric | Value |
|--------|-------|
| Pass rate | **14/14 (100.0%)** |
| Failed | 0 |
| Timed out | 0 |
| Mean time per trial | 181.8s |
| Total wall clock | 2544.8s |
| Total tokens | 337,739 |
| Total turns | 94 |
| Native executor | 14/14 |
| Own service instance | 14/14 |
| Federation pull | 14/14 |
| Federation push | 14/14 |

## Per-Task Results

| Task | Difficulty | Pass Rate | Mean Time |
|------|-----------|-----------|----------|
| file-ops | easy | 2/2 (100%) | 125.0s |
| text-processing | easy | 2/2 (100%) | 213.6s |
| debugging | medium | 2/2 (100%) | 131.4s |
| shell-scripting | medium | 2/2 (100%) | 164.3s |
| data-processing | medium | 2/2 (100%) | 197.0s |
| algorithm | hard | 2/2 (100%) | 214.3s |
| ml | hard | 2/2 (100%) | 226.7s |

## Per-Difficulty Results

| Difficulty | Pass Rate |
|-----------|----------|
| easy | 4/4 (100%) |
| medium | 6/6 (100%) |
| hard | 4/4 (100%) |

## Trial Details

| Trial | Task | Rep | Status | Time | Turns | Tokens |
|-------|------|-----|--------|------|-------|--------|
| aprime-file-ops-r0 | file-ops | 0 | PASS | 119.5s | 6 | 16,044 |
| aprime-file-ops-r1 | file-ops | 1 | PASS | 130.5s | 6 | 19,121 |
| aprime-text-processing-r0 | text-processing | 0 | PASS | 181.7s | 6 | 12,483 |
| aprime-text-processing-r1 | text-processing | 1 | PASS | 245.5s | 6 | 13,213 |
| aprime-debugging-r0 | debugging | 0 | PASS | 133.7s | 10 | 30,425 |
| aprime-debugging-r1 | debugging | 1 | PASS | 129.0s | 9 | 25,622 |
| aprime-shell-scripting-r0 | shell-scripting | 0 | PASS | 192.8s | 5 | 18,885 |
| aprime-shell-scripting-r1 | shell-scripting | 1 | PASS | 135.9s | 5 | 18,750 |
| aprime-data-processing-r0 | data-processing | 0 | PASS | 262.1s | 7 | 29,018 |
| aprime-data-processing-r1 | data-processing | 1 | PASS | 131.9s | 7 | 24,799 |
| aprime-algorithm-r0 | algorithm | 0 | PASS | 246.4s | 7 | 20,510 |
| aprime-algorithm-r1 | algorithm | 1 | PASS | 182.2s | 5 | 16,820 |
| aprime-ml-r0 | ml | 0 | PASS | 244.4s | 5 | 21,917 |
| aprime-ml-r1 | ml | 1 | PASS | 209.0s | 10 | 70,132 |

## Validation Checklist

- [x] At least 10 trials ran to completion
- [x] Each trial used native WG executor
- [x] Each trial had its own wg service instance
- [x] Federation pull verified for each trial
- [x] Federation push verified for each trial
- [x] Results summary with pass/fail counts, mean score, timing
- [x] Failures documented with root cause
