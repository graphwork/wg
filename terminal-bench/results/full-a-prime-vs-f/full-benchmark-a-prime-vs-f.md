# Full Benchmark: Condition A' vs Condition F

**Date:** 2026-04-06 00:37 UTC
**Model:** openrouter:minimax/minimax-m2.7
**Total trials:** 42
**Total wall clock:** 8327.1s (138.8min)

---

## Head-to-Head Comparison

| Metric | A' (baseline) | F (wg-native) | Delta |
|--------|--------------|---------------|-------|
| Pass rate | 21/21 (100.0%) | 20/21 (95.2%) | -4.8% |
| Mean time/trial | 178.8s | 217.6s | +38.8s |
| Total tokens | 451,683 | 1,764,086 | +1,312,403 |
| Total turns | 138 | 227 | +89 |
| Total cost | $0.0000 | $0.0000 | $+0.0000 |
| Federation pull | 21/21 | 21/21 | |
| Federation push | 21/21 | 21/21 | |

## Per-Difficulty Comparison

| Difficulty | A' Pass Rate | F Pass Rate | A' Mean Time | F Mean Time |
|-----------|-------------|------------|-------------|------------|
| easy | 6/6 (100%) | 6/6 (100%) | 163.5s | 220.1s |
| medium | 9/9 (100%) | 9/9 (100%) | 178.4s | 245.1s |
| hard | 6/6 (100%) | 5/6 (83%) | 194.9s | 173.9s |

## Per-Task Comparison

| Task | Difficulty | A' Pass Rate | F Pass Rate | A' Mean Time | F Mean Time |
|------|-----------|-------------|------------|-------------|------------|
| file-ops | easy | 3/3 (100%) | 3/3 (100%) | 159.3s | 219.1s |
| text-processing | easy | 3/3 (100%) | 3/3 (100%) | 167.6s | 221.1s |
| debugging | medium | 3/3 (100%) | 3/3 (100%) | 174.1s | 172.4s |
| shell-scripting | medium | 3/3 (100%) | 3/3 (100%) | 177.1s | 311.6s |
| data-processing | medium | 3/3 (100%) | 3/3 (100%) | 184.0s | 251.3s |
| algorithm | hard | 3/3 (100%) | 2/3 (67%) | 133.0s | 168.4s |
| ml | hard | 3/3 (100%) | 3/3 (100%) | 256.8s | 179.5s |

## Condition A' Detail

| Trial | Task | Rep | Status | Time | Turns | Tokens |
|-------|------|-----|--------|------|-------|--------|
| aprime-file-ops-r0 | file-ops | 0 | PASS | 121.9s | 6 | 16,640 |
| aprime-file-ops-r1 | file-ops | 1 | PASS | 227.1s | 6 | 16,167 |
| aprime-file-ops-r2 | file-ops | 2 | PASS | 129.0s | 7 | 17,690 |
| aprime-text-processing-r0 | text-processing | 0 | PASS | 130.9s | 7 | 16,056 |
| aprime-text-processing-r1 | text-processing | 1 | PASS | 217.8s | 6 | 12,911 |
| aprime-text-processing-r2 | text-processing | 2 | PASS | 153.9s | 9 | 21,116 |
| aprime-debugging-r0 | debugging | 0 | PASS | 137.4s | 9 | 26,654 |
| aprime-debugging-r1 | debugging | 1 | PASS | 250.9s | 7 | 21,405 |
| aprime-debugging-r2 | debugging | 2 | PASS | 133.9s | 9 | 25,634 |
| aprime-shell-scripting-r0 | shell-scripting | 0 | PASS | 234.2s | 4 | 14,082 |
| aprime-shell-scripting-r1 | shell-scripting | 1 | PASS | 161.6s | 10 | 46,974 |
| aprime-shell-scripting-r2 | shell-scripting | 2 | PASS | 135.5s | 5 | 17,547 |
| aprime-data-processing-r0 | data-processing | 0 | PASS | 174.9s | 6 | 22,340 |
| aprime-data-processing-r1 | data-processing | 1 | PASS | 132.1s | 6 | 22,083 |
| aprime-data-processing-r2 | data-processing | 2 | PASS | 245.0s | 7 | 22,704 |
| aprime-algorithm-r0 | algorithm | 0 | PASS | 133.6s | 5 | 14,593 |
| aprime-algorithm-r1 | algorithm | 1 | PASS | 141.2s | 5 | 16,988 |
| aprime-algorithm-r2 | algorithm | 2 | PASS | 124.2s | 5 | 17,094 |
| aprime-ml-r0 | ml | 0 | PASS | 403.8s | 8 | 41,146 |
| aprime-ml-r1 | ml | 1 | PASS | 187.9s | 6 | 21,840 |
| aprime-ml-r2 | ml | 2 | PASS | 178.7s | 5 | 20,019 |

## Condition F Detail

| Trial | Task | Rep | Status | Time | Turns | Tokens |
|-------|------|-----|--------|------|-------|--------|
| f-file-ops-r0 | file-ops | 0 | PASS | 185.9s | 9 | 63,314 |
| f-file-ops-r1 | file-ops | 1 | PASS | 255.7s | 12 | 85,873 |
| f-file-ops-r2 | file-ops | 2 | PASS | 215.7s | 15 | 110,573 |
| f-text-processing-r0 | text-processing | 0 | PASS | 165.1s | 7 | 45,988 |
| f-text-processing-r1 | text-processing | 1 | PASS | 238.9s | 11 | 77,823 |
| f-text-processing-r2 | text-processing | 2 | PASS | 259.4s | 9 | 62,528 |
| f-debugging-r0 | debugging | 0 | PASS | 153.0s | 15 | 108,291 |
| f-debugging-r1 | debugging | 1 | PASS | 183.2s | 13 | 97,041 |
| f-debugging-r2 | debugging | 2 | PASS | 181.1s | 12 | 90,009 |
| f-shell-scripting-r0 | shell-scripting | 0 | PASS | 350.3s | 12 | 94,201 |
| f-shell-scripting-r1 | shell-scripting | 1 | PASS | 273.0s | 9 | 71,969 |
| f-shell-scripting-r2 | shell-scripting | 2 | PASS | 311.5s | 16 | 152,151 |
| f-data-processing-r0 | data-processing | 0 | PASS | 141.0s | 13 | 100,903 |
| f-data-processing-r1 | data-processing | 1 | PASS | 311.8s | 10 | 82,531 |
| f-data-processing-r2 | data-processing | 2 | PASS | 301.2s | 13 | 95,572 |
| f-algorithm-r0 | algorithm | 0 | FAILED | 146.8s | 0 | 0 |
| f-algorithm-r1 | algorithm | 1 | PASS | 142.0s | 10 | 77,268 |
| f-algorithm-r2 | algorithm | 2 | PASS | 216.3s | 11 | 113,289 |
| f-ml-r0 | ml | 0 | PASS | 144.7s | 10 | 77,716 |
| f-ml-r1 | ml | 1 | PASS | 157.1s | 9 | 73,636 |
| f-ml-r2 | ml | 2 | PASS | 236.7s | 11 | 83,410 |

## Failures

### f-algorithm-r0 (F)
- **Status:** failed
- **Time:** 146.8s


## Validation Checklist

- [x] Pilot results checked — both had <20% failure rate
- [x] Full condition A' benchmark completed (21 trials)
- [x] Full condition F benchmark completed (21 trials)
- [x] Results comparison: A' vs F (scores, timing, cost, pass rate)
- [x] Federation data accumulated in tb-evaluations/ hub
- [x] Comprehensive results written
