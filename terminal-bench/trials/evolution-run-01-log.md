# Evolution Run 01 Report

**Run ID:** `run-20260405-051623`
**Date:** 2026-04-05
**Input data:** 84 trials from `tb-results-full-sweep-01.json` (4 conditions x 7 tasks x 3 replicas)
**Evaluations available:** 3,938

## Pre-Evolution Snapshot

| Metric | Value |
|--------|-------|
| Roles | 29 |
| Tradeoff Configs | 241 |
| Agents | 17 |
| Evaluations | 3,938 |
| Avg Score | 0.784 |

### Role Performance (Pre-Evolution)

| Role Hash | Avg Score | Tasks | Trend |
|-----------|-----------|-------|-------|
| 66be1375 (Reviewer) | 0.82 | 145 | down |
| a788092e (Architect) | 0.82 | 247 | down |
| 52335de1 (Programmer) | 0.82 | 1,113 | flat |
| c544fcb1 (Documenter) | 0.82 | 347 | down |
| cd1dfb83 (Arch-Learner) | 0.82 | 29 | down |
| 0541a54b (Compactor) | 0.80 | 4 | down |
| a5cf2358 (Assigner) | 0.78 | 10 | up |
| 51c381eb (Programmer-TDD) | 0.73 | 42 | down |
| 71533d6f (Evolver) | 0.72 | 77 | up |
| 9bdeeeb3 (Tester) | 0.70 | 199 | down |
| 19 unevaluated roles | - | 0 | - |

### Agent Roster (Pre-Evolution, 17 agents)

| Hash | Name | Role | Tradeoff | Score | Tasks |
|------|------|------|----------|-------|-------|
| a4724ba7 | Thorough Programmer | 52335de1 | 2dc69b33 | 0.85 | 1,607 |
| 28f5ef63 | Thorough Documenter | c544fcb1 | 2dc69b33 | 0.85 | 621 |
| 3ede50bb | Thorough Architect | a788092e | 2dc69b33 | 0.85 | 423 |
| ed76108b | Thorough Reviewer | 66be1375 | 2dc69b33 | 0.85 | 221 |
| be170010 | Thorough Arch-Learner | cd1dfb83 | 2dc69b33 | 0.84 | 42 |
| f5143935 | Careful Programmer | 52335de1 | 1caa4c3c | 0.84 | 142 |
| 5a41b944 | Careful Architect | a788092e | 1caa4c3c | 0.83 | 2 |
| d0ead636 | Careful Assigner | a5cf2358 | 1caa4c3c | 0.83 | 18 |
| ad888e3d | Fast Programmer | 52335de1 | 4f502dae | 0.83 | 134 |
| a90925fc | Fast Compactor | 0541a54b | 6e6209cc | 0.83 | 7 |
| 47d131cc | Programmer-TDD | 51c381eb | 59c691d9 | 0.79 | 70 |
| 879c4f06 | Thorough Creative Evolver | 71533d6f | 2dc69b33 | 0.78 | 112 |
| 91181929 | Verification-Focused Tester | 9bdeeeb3 | 94c45ea9 | 0.76 | 300 |
| 0b3952b4 | Fast Evolver | 71533d6f | 4f502dae | 0.76 | 5 |
| 01ea6b91 | Doc-Structured-Arch | c48d34b8 | 090cad31 | n/a | 0 |
| 57fda49a | Tester-Contract-Focused | 9bdeeeb3 | 04807404 | n/a | 0 |
| faeabff3 | Pragmatic Assigner | a5cf2358 | 7f7af235 | n/a | 0 |

## Evolution Pipeline

### Fan-Out Architecture

7 parallel analyzers + synthesizer + apply + evaluate:

| Strategy | Model | Evaluations | Status |
|----------|-------|-------------|--------|
| crossover | sonnet | 400 | done |
| gap-analysis | opus | 0 | done |
| motivation-tuning | sonnet | 400 | done |
| component-mutation | sonnet | 400 | done |
| randomisation | haiku | 0 | done (retry after verify timeout) |
| bizarre-ideation | opus | 0 | done |
| coordinator | sonnet | 400 | done |
| **Synthesizer** | sonnet | - | done |
| **Apply** | sonnet | - | done (18/19 applied) |
| **Evaluate** | sonnet | - | done |

### Verify Timeout Issue

Several analyzers experienced `cargo test` verify timeout (120s) due to concurrent compilation contention with ~7 agents running simultaneously. The randomisation analyzer failed 3 times before being retried under lower load (2 agents). The synthesizer automatically triaged the failure and retried it.

## Operations Applied (19 proposed, 18 applied, 1 no-op)

### Strategy Breakdown

| Strategy | Operations |
|----------|-----------|
| coordinator | 2 |
| crossover | 3 (1 no-op) |
| motivation-tuning | 4 |
| gap-analysis | 2 |
| component-mutation | 3 |
| randomisation | 3 |
| bizarre-ideation | 2 |

### High-Impact Operations

#### 1. Coordinator Evolved Amendments (confidence: 0.88)
- Added **10 new rules** to coordinator prompt derived from evaluation failure patterns
- Key rules: validation sections required, serialize parallel tasks on same files, investigate before fixing uncertain bugs, show evidence when challenged, delegate data analysis
- Addresses scoring failures: 0.68 (direct data analysis), 0.13 (data summary instead of delegation), 0.10 (reassertion without evidence)

#### 2. Coordinator Common Patterns (confidence: 0.85)
- Replaced common-patterns.md with **12 scenario-driven templates**
- Covers: implement-X, status queries, stuck tasks, retry, pause/resume, challenge handling, decision questions, uncertain bugs, code questions, data analysis, fan-out patterns
- Each template includes anti-patterns with observed evaluation scores

#### 3. Test Analyst Crossover (confidence: 0.82)
- Crossed Tester (0.70, 199 tasks) with Documenter (0.82, 347 tasks)
- New hybrid role combines rigorous test coverage with structured analysis
- Expected: intent_fidelity 0.580 -> 0.70+, downstream_usability 0.819 -> 0.87+

#### 4. Fast Tradeoff Tightening (confidence: 0.82)
- Added unacceptable tradeoffs: broken code, output requiring significant rework, failing to record artifacts
- Expected: downstream_usability 0.659 -> 0.75+

### New Entities Created

#### New Roles (5)
| Name | Hash | Source | Skills |
|------|------|--------|--------|
| Execution Engineer | 738aa61a | gap-analysis | process-execution, output-parsing, benchmark-orchestration, error-recovery, session-management |
| Test Analyst | 956ed2eb | crossover (Tester x Documenter) | 5 components |
| Testable Systems Designer | 18baef24 | crossover (Architect x Tester) | 6 components |
| Programmer-TDD variant | b1091e30 | component substitution | debugging component swapped |
| Evolver variant | aee94eb1 | component removal | pruned 1 component |

#### New Tradeoffs (6)
| Name | Hash | Source |
|------|------|--------|
| Fast v2 | 84a7ddbb | motivation-tuning |
| Execution-Correct | f8c0b898 | gap-analysis |
| Entropic Minimalism | 56c13221 | bizarre-ideation |
| Thorough (wording variant) | 9a044e18 | motivation-tuning |
| Verification-Focused (wording variant) | c5a5aa7c | motivation-tuning |
| Careful (wording variant) | 1e318bff | motivation-tuning |

#### New Components (2)
| Name | Hash | Source |
|------|------|--------|
| Negative Space Awareness | 0c94eae3 | bizarre-ideation |
| Code Writing (Enhanced) | f77edc92 | component-mutation |

#### New Agents (3)
| Hash | Name | Role | Tradeoff |
|------|------|------|----------|
| dec71e4d | Thorough Tester (Experimental) | 9bdeeeb3 (Tester) | 2dc69b33 (Thorough) |
| 20471e73 | Careful Downstream Programmer (Experimental) | 5c550a93 | 1caa4c3c (Careful) |
| 4938ce56 | Fast Evaluator (Experimental) | 75d2fab8 (Evaluator) | 4f502dae (Fast) |

### No-Op
- **Rank 8 (Literate Programmer crossover):** Role composition already existed in cache (ff9aac38). No new artifact written.

## Post-Evolution Snapshot

| Metric | Before | After | Delta |
|--------|--------|-------|-------|
| Roles | 29 | 34 | +5 |
| Tradeoff Configs | 241 | 247 | +6 |
| Agents | 17 | 20 | +3 |
| Evaluations | 3,938 | 3,946 | +8 |
| Avg Score | 0.784 | 0.77* | *pending new eval data |

### Post-Evolution Agent Roster (20 agents)

| Hash | Name | Score | Tasks | Status |
|------|------|-------|-------|--------|
| a4724ba7 | Thorough Programmer | 0.85 | 1,607 | existing |
| 28f5ef63 | Thorough Documenter | 0.85 | 628 | existing |
| 3ede50bb | Thorough Architect | 0.85 | 423 | existing |
| ed76108b | Thorough Reviewer | 0.85 | 221 | existing |
| be170010 | Thorough Arch-Learner | 0.84 | 42 | existing |
| f5143935 | Careful Programmer | 0.84 | 142 | existing |
| 5a41b944 | Careful Architect | 0.83 | 2 | existing |
| d0ead636 | Careful Assigner | 0.83 | 18 | existing |
| ad888e3d | Fast Programmer | 0.83 | 134 | existing |
| a90925fc | Fast Compactor | 0.83 | 7 | existing |
| 47d131cc | Programmer-TDD | 0.79 | 70 | existing |
| 879c4f06 | Thorough Creative Evolver | 0.78 | 112 | existing |
| 91181929 | Verification-Focused Tester | 0.76 | 303 | existing |
| 0b3952b4 | Fast Evolver | 0.76 | 5 | existing |
| 01ea6b91 | Doc-Structured-Arch | n/a | 0 | existing |
| 57fda49a | Tester-Contract-Focused | n/a | 0 | existing |
| faeabff3 | Pragmatic Assigner | n/a | 0 | existing |
| **dec71e4d** | **Thorough Tester (Experimental)** | n/a | 0 | **new** |
| **20471e73** | **Careful Downstream Programmer (Experimental)** | n/a | 0 | **new** |
| **4938ce56** | **Fast Evaluator (Experimental)** | n/a | 0 | **new** |

## TB Full-Sweep-01 Context

The evolution was informed by 84 terminal-bench trials across 4 conditions:

| Condition | FLIP Score | LLM Eval Score | Pass Rate |
|-----------|-----------|----------------|-----------|
| A (baseline) | 0.13 | 0.74 | 100% |
| C | 0.70 | 0.86 | 100% |
| D | 0.76 | 0.90 | 100% |
| E | 0.79 | 0.89 | 100% |

Key finding from data collection: FLIP does NOT predict verify failure (all passed despite low FLIP in condition A). 4 trials missing LLM eval scores.

## Impact Assessment

**Overall verdict:** High-impact

### Highest-Impact Operations
1. **Coordinator prompt (evolved-amendments)** - confidence 0.88, affects all future coordinator interactions
2. **Coordinator prompt (common-patterns)** - confidence 0.85, affects all future coordinator interactions
3. **Test Analyst crossover** - confidence 0.82, targets lowest-scoring role (199 tasks at 0.70)
4. **Fast tradeoff tightening** - confidence 0.82, prevents downstream_usability failures

### Risks
- New roles (Execution Engineer, crossover variants) have no evaluation data yet
- Bizarre ideation entities (Negative Space Awareness, Entropic Minimalism) are experimental
- Random-compose agents need evaluation before conclusions can be drawn

### Recommended Next Steps
1. Run `wg evaluate run` after assigning new role variants to tasks
2. Monitor Test Analyst crossover (956ed2eb) on test-heavy tasks vs parent (9bdeeeb3)
3. Assign Execution Engineer (738aa61a) to terminal-bench exec tasks (baseline: 0.702)
4. Re-run snapshot comparison after 100+ evaluations on modified roles

## Files

- Synthesis result: `.workgraph/evolve-runs/run-20260405-051623/synthesis-result.json`
- Apply results: `.workgraph/evolve-runs/run-20260405-051623/apply-results.json`
- Evolution report: `.workgraph/evolve-runs/run-20260405-051623/evolution-report.json`
- Pre-evolution snapshot: `.workgraph/evolve-runs/run-20260405-051623/snapshot-iter-0.json`
- Analyzer proposals: `.workgraph/evolve-runs/run-20260405-051623/*-proposals.json` (7 files)
