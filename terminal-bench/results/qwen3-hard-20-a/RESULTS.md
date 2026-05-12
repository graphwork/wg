# TB Stress Test: Qwen3-Coder-30B — All 18 Tasks — Condition A

**Date:** 2026-04-13
**Model:** Qwen3-Coder-30B (local:qwen3-coder-30b)
**Endpoint:** http://lambda01:30000/v1 (SGLang, RTX 6000 Ada 48GB)
**Context Window:** 32,768 tokens
**Condition:** A (agent-only, no wg decomposition)

## Executive Summary

**Pass rate: 13/18 (72%)**

| Difficulty | Passed | Total | Rate |
|-----------|--------|-------|------|
| Hard | 8 | 13 | 62% |
| Medium | 3 | 3 | 100% |
| Easy | 2 | 2 | 100% |

All 5 failures share a single root cause: **context window overflow**. The model hits the 32k token limit when tasks require iterative debugging with verbose tool outputs, and emergency compaction cannot recover enough context to continue productively.

## Failure Mode Analysis

### The Only Failure Mode: Context Overflow (5/5 failures)

Every failure follows the same pattern:
1. Model begins work, generates tool calls with verbose output
2. Conversation history accumulates (especially with large file reads, hexdumps, debug prints)
3. At ~16k input tokens per turn, the model hits: `API error 400: Requested token count exceeds the model's maximum context length of 32768 tokens`
4. Emergency compaction triggers but doesn't drop enough messages (e.g., "57 → 57 messages")
5. Model loses context of what it already tried, re-attempts the same failing approach
6. Eventually exhausts turns or fails verification

**Failed tasks (all hit context limit at ~49.5% utilization):**

| Task | Turns | Max Input | Ctx% | Failure Detail |
|------|-------|-----------|------|----------------|
| cobol-modernization | 34 | 16,246 | 49.6% | Stuck in COBOL fixed-width parsing loop, couldn't resolve off-by-one in field positions |
| multi-source-data-merger | 24 | 16,144 | 49.3% | Detected only 2/4 conflicts; after context overflow lost track of which conflicts were found |
| financial-document-processor | 36 | 16,282 | 49.7% | Repeated context overflow during multi-step extraction pipeline |
| constraints-scheduling | 25 | 16,300 | 49.7% | ICS parsing + constraint solving exceeded context budget |
| iterative-test-fix | 38 | 16,243 | 49.6% | 6 interrelated bugs + 15 tests: conversation too long for iterative fix cycle |

**Key observation:** The ~49.5% utilization ceiling corresponds to input_tokens ≈ 16,200 hitting the 32k limit (16,200 input + 16,384 max_completion = 32,584 ≈ 32,768). Tasks that stay under ~14k max input tokens all pass.

### Passing Task Characteristics

| Task | Turns | Max Input | Ctx% | Category |
|------|-------|-----------|------|----------|
| configure-git-webserver | 51 | 11,627 | 35.5% | pipeline (most complex passing task) |
| mailman | 50 | 11,106 | 33.9% | pipeline |
| multi-module-type-migration | 32 | 11,163 | 34.1% | cascading |
| fix-code-vulnerability | 17 | 13,934 | 42.5% | multi-file |
| build-cython-ext | 19 | 9,376 | 28.6% | pipeline |
| ml | 14 | 9,897 | 30.2% | algorithm |
| algorithm | 6 | 4,359 | 13.3% | algorithm |
| sysadmin | 14 | 6,436 | 19.6% | systems |

Passing tasks stay below ~42% context utilization. The 35-42% range is the danger zone — tasks succeed but are close to the limit.

## Context Management Behavior

- **Emergency compaction** triggers on context overflow but is largely ineffective — it reports compacting "57 → 57 messages" (no messages dropped)
- **No sliding window**: The native executor does not implement message dropping; it relies on the model's context window
- **Token growth pattern**: Input tokens grow roughly linearly with turns. At ~500 tokens/turn growth, a task with 30+ turns will hit the wall
- **Verbose tool outputs** are the primary context consumer — file reads, debug prints, and hexdumps dominate

## Comparison with 10-Task Pilot

The pilot (10 tasks, 2 easy / 3 medium / 5 hard) went **10/10 (100%)**. The pilot's 5 hard tasks were: algorithm, ml, sysadmin, configure-git-webserver, mailman — all of which passed again here.

The 5 NEW hard-benchmark tasks not in the pilot (multi-source-data-merger, financial-document-processor, cobol-modernization, constraints-scheduling, iterative-test-fix) went **0/5 (0%)**. These tasks are structurally harder: they involve multi-step data transformations with iterative debugging, producing longer conversations that blow the context window.

**build-cython-ext** (new, passed), **fix-code-vulnerability** (new, passed), and **multi-module-type-migration** (new, passed) are the 3 new hard tasks that succeeded — their workflows are more structured and don't require as much iterative debugging.

## Recommendations

1. **Context window is the binding constraint.** A 32k model can handle TB tasks that resolve in <30 turns with <14k peak input tokens. Beyond that, context overflow is near-certain.

2. **Longer context models would help.** The failures are not due to model capability but context limits. A 128k context model would likely pass all 18 tasks.

3. **Context management improvements needed:**
   - Sliding window message dropping (oldest messages first)
   - Summarization of verbose tool outputs before they enter the conversation
   - Limiting tool output size (e.g., truncate file reads to 200 lines)
   - More aggressive emergency compaction (actually drop messages, not just report)

4. **For Condition G comparison:** The 5 failing tasks are ideal candidates for graph decomposition — breaking multi-step tasks into subtasks would keep each agent's context short.

## Task Selection Note

The task description requested 20 hardest tasks, but only 18 local task definitions exist (the full TB 2.0 catalog has 89 tasks but the remaining 71 require Harbor/Docker infrastructure). All 18 available tasks were run.

## Raw Data

- `combined_summary.json` — full results with per-task metrics
- `qwen3-hard-20-a-{task}/workgraph_state/` — saved wg state per trial
- `qwen3-hard-20-a-{task}/workgraph_state/agents/agent-1/output.log` — full agent conversation
