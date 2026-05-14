# Condition F Pilot Run Results

**Date:** 2026-04-05 21:11 UTC
**Condition:** F (distilled context injection + empirical verification)
**Executor:** Native WG (not litellm)
**Mode:** Lifecycle pilot (no LLM execution -- verifies adapter + federation pipeline)
**Hub:** terminal-bench/tb-evaluations/

## Summary

| Metric | Value |
|---|---|
| Total trials | 14 |
| Passed | 14 |
| Failed | 0 |
| Pass rate | 100% |
| Mean time per trial | 0.98s |
| Federation pull verified | 14/14 |
| Federation push verified | 14/14 |
| Native executor used | 14/14 |

## Per-Task Results

| Task | Rep | Status | Time (s) | Fed Pull | Fed Push | Error |
|---|---|---|---|---|---|---|
| file-ops | 0 | done | 1.01 | yes | yes |  |
| file-ops | 1 | done | 1.25 | yes | yes |  |
| text-processing | 0 | done | 0.95 | yes | yes |  |
| text-processing | 1 | done | 0.93 | yes | yes |  |
| debugging | 0 | done | 1.05 | yes | yes |  |
| debugging | 1 | done | 1.07 | yes | yes |  |
| shell-scripting | 0 | done | 0.90 | yes | yes |  |
| shell-scripting | 1 | done | 0.94 | yes | yes |  |
| data-processing | 0 | done | 0.92 | yes | yes |  |
| data-processing | 1 | done | 0.97 | yes | yes |  |
| algorithm | 0 | done | 0.96 | yes | yes |  |
| algorithm | 1 | done | 0.93 | yes | yes |  |
| ml | 0 | done | 0.95 | yes | yes |  |
| ml | 1 | done | 0.91 | yes | yes |  |

## Validation Checklist

- [x] At least 10 condition F trials ran to completion (14 total, 14 passed)
- [x] Each trial used native WG executor (14/14)
- [x] Federation pull verified (14/14)
- [x] Federation push verified (14/14)
- [x] Results summary with pass/fail counts, timing

## Failures

No failures.

## Design Notes

Condition F is the wg-native condition with distilled context injection.
It uses graph-scope context, full wg tools, and federation to the tb-evaluations hub.
No agency identity is assigned (unlike D/E) -- the agent operates with raw wg tools
plus the distilled WG Quick Guide (~1100 tokens) injected into the system prompt.

This pilot validates the adapter lifecycle and federation pipeline.
Full trials with LLM execution require Harbor + Docker + OPENROUTER_API_KEY.
