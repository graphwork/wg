# Pilot: Condition A vs G — Claude Haiku

**Date:** 2026-04-13 02:10 UTC
**Model:** claude:haiku
**Tasks:** 5
**Total wall clock:** 107.1s (1.8min)

## Conditions

- **Condition A (wg):** Workgraph-coordinated. Isolated wg service + native-exec agent.
- **Condition G (raw):** Raw Claude Code. `claude --model haiku -p` with no workgraph.

## Results

| Task | Difficulty | Cond A (wg) | Cond A Time | Cond G (raw) | Cond G Time | Delta |
|------|-----------|-------------|------------|--------------|------------|-------|
| text-processing | easy | PASS | 27.1s | PASS | 31.4s | tie |
| debugging | medium | PASS | 42.2s | PASS | 23.7s | tie |
| data-processing | medium | PASS | 42.1s | PASS | 27.4s | tie |
| algorithm | hard | PASS | 47.1s | PASS | 43.5s | tie |
| ml | hard | PASS | 36.4s | PASS | 39.1s | tie |

**Summary:** A wins 0, G wins 0, ties 5

## Per-Condition Summary

| Condition | Pass Rate | Mean Time | Total Tokens | Cost |
|-----------|-----------|-----------|-------------|------|
| A (wg) | 5/5 (100%) | 39.0s | 0 | $0.0000 |
| G (raw) | 5/5 (100%) | 33.0s | 20,569 | $0.2460 |

## Conclusion

Both conditions performed **equally** — workgraph was neutral on this task set.
Condition G was faster on average (33.0s vs 39.0s).

## Trial Details

| Trial | Condition | Task | Status | Time | Tokens | Failure Mode |
|-------|-----------|------|--------|------|--------|--------------|
| condA-text-processing | A (wg) | text-processing | PASS | 27.1s | 0 | N/A |
| condA-debugging | A (wg) | debugging | PASS | 42.2s | 0 | N/A |
| condA-data-processing | A (wg) | data-processing | PASS | 42.1s | 0 | N/A |
| condA-algorithm | A (wg) | algorithm | PASS | 47.1s | 0 | N/A |
| condA-ml | A (wg) | ml | PASS | 36.4s | 0 | N/A |
| condG-text-processing | G (raw) | text-processing | PASS | 31.4s | 3,411 | N/A |
| condG-debugging | G (raw) | debugging | PASS | 23.7s | 2,185 | N/A |
| condG-data-processing | G (raw) | data-processing | PASS | 27.4s | 3,540 | N/A |
| condG-algorithm | G (raw) | algorithm | PASS | 43.5s | 6,082 | N/A |
| condG-ml | G (raw) | ml | PASS | 39.1s | 5,351 | N/A |
