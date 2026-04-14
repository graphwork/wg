# TB Retest: Smart Fanout G vs Original G vs A

**Date:** 2026-04-14 06:38 UTC
**Model:** local:qwen3-coder-30b
**Endpoint:** http://lambda01:30000/v1
**Context window:** 32768 tokens
**Task count:** 10
**Tasks:** cobol-modernization, constraints-scheduling, multi-source-data-merger, algorithm, ml, fix-code-vulnerability, configure-git-webserver, debugging, data-processing, file-ops

## Summary Comparison

| Metric | Condition A | Condition G-orig | Condition G-smart |
|--------|------------|------------------|-------------------|
| Pass rate | 70% (7/10) | 80% (8/10) | 60% (6/10) |
| Median time (s) | 0 | 1025.41 | 67.98 |
| Decomposition rate | 0% | 100% | 0% |
| Avg subtasks/trial | 1.0 | 16.9 | 1.0 |
| Max agents | 1 | 4 | 4 |

## Per-Task Head-to-Head

| Task | Diff | A | G-orig | G-smart | A time | G-orig time | G-smart time | G-smart subtasks | G-smart decision |
|------|------|---|--------|---------|--------|-------------|--------------|------------------|------------------|
| cobol-modernization | hard | FAIL | FAIL | FAIL | 0s | 1206s | 68s | 1 |  |
| constraints-scheduling | hard | FAIL | FAIL | FAIL | 0s | 1204s | 74s | 1 | direct |
| multi-source-data-merger | hard | FAIL | PASS | PASS | 0s | 1205s | 63s | 1 |  |
| algorithm | hard | PASS | PASS | PASS | 0s | 723s | 33s | 1 |  |
| ml | hard | PASS | PASS | PASS | 0s | 118s | 73s | 1 | direct |
| fix-code-vulnerability | hard | PASS | PASS | PASS | 0s | 1025s | 73s | 1 |  |
| configure-git-webserver | hard | PASS | PASS | FAIL | 0s | 1204s | 98s | 1 | direct |
| debugging | medium | PASS | PASS | PASS | 0s | 908s | 27s | 1 |  |
| data-processing | medium | PASS | PASS | PASS | 0s | 902s | 43s | 1 |  |
| file-ops | easy | PASS | PASS | FAIL | 0s | 560s | 31s | 1 | direct |

## Failure Mode Breakdown

**A:** {'context_overflow': 3, 'success': 7}

**G-original:** {'rate_limit': 1, 'timeout': 1, 'success': 8}

**G-smart:** {'wrong_answer': 3, 'success': 6, 'status_done': 1}

## G-smart Fanout Decisions (detailed)

**cobol-modernization** (reward=0.0):
  - No FANOUT_DECISION logged

**constraints-scheduling** (reward=0.0):
  - Initial: FANOUT_DECISION: direct — The task is a single logical unit of work involving parsing ICS files, finding time slots, and generating meetings. It touches 3-4 files total and has clear requirements.

**multi-source-data-merger** (reward=1.0):
  - No FANOUT_DECISION logged

**algorithm** (reward=1.0):
  - No FANOUT_DECISION logged

**ml** (reward=1.0):
  - Initial: FANOUT_DECISION: direct — Task is well-scoped, single logical unit with clear requirements. Only need to implement one Python file with specific functionality.

**fix-code-vulnerability** (reward=1.0):
  - No FANOUT_DECISION logged

**configure-git-webserver** (reward=0.0):
  - Initial: FANOUT_DECISION: direct — Task is a single logical unit of work that requires modifying 3-4 files (git repo setup, post-receive hook, web server, test workflow)

**debugging** (reward=1.0):
  - No FANOUT_DECISION logged

**data-processing** (reward=1.0):
  - No FANOUT_DECISION logged

**file-ops** (reward=0.0):
  - Initial: FANOUT_DECISION: direct — Task is straightforward file creation with clear structure and validation criteria

## Analysis & Verdict

- **G-smart vs A delta:** -10%
- **G-original vs A delta:** +10%
- **G-smart vs G-original delta:** -20%

- **Overhead reduction (G-smart vs G-original):** 93% (median time 1025s → 68s)
- **G-smart overhead vs A:** +68s median