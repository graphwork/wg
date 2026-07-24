# Design: Smart Fanout Calculus for Condition G

**Task:** tb-design-smart-fanout
**Date:** 2026-04-13
**Based on:** TB-CONDITION-G-FANOUT-ANALYSIS.md (tb-research-g-fanout)

---

## Problem Statement

Condition G's "always decompose" meta-prompt forces every task through an architect→coordinator→worker pipeline regardless of task complexity. Research shows this adds 4-8 minutes of overhead per trial, harms 72% of tasks that a single agent handles directly, and produces worse overall results than Condition A (no decomposition).

The fix: replace the unconditional decomposition mandate with an explicit cost/benefit calculus that the seed agent applies *before* deciding whether to decompose.

---

## Design: Try-First, Decompose-If-Needed

### Core Principle

The seed agent should **attempt the task directly first**. Decomposition is a fallback for when direct execution fails or is predicted to fail based on observable signals — not the default path.

This addresses the fundamental tension identified in research: "understanding the problem well enough to decompose it often means you're already most of the way to solving it."

### The Decision Sequence

The seed agent follows this sequence for every task:

```
PHASE 1: TRIAGE (< 2 minutes)
├── Read the task instruction
├── Scan the working directory (ls, ls tests/)
├── Count: files to modify, test cases, instruction word count
├── Estimate: is this a 1-agent task or a multi-agent task?
│
├── IF instruction < 300 words
│   AND touches ≤ 2 files
│   AND test suite has ≤ 5 tests
│   → IMPLEMENT DIRECTLY (skip to Phase 2a)
│
├── IF instruction > 500 words
│   AND references > 3 distinct files to modify
│   AND has clear independent sub-problems (different files, no ordering)
│   → DECOMPOSE (skip to Phase 2b)
│
└── OTHERWISE → IMPLEMENT DIRECTLY (default: try first)

PHASE 2a: DIRECT IMPLEMENTATION
├── Implement the solution yourself
├── Run the test suite
├── If tests pass → done
├── If you hit context pressure (losing track of what you've done,
│   re-reading files you already read, forgetting earlier edits)
│   → Switch to Phase 2b: create subtasks for remaining work
└── If tests fail → iterate (you have the context to fix it)

PHASE 2b: DECOMPOSITION (only when triggered)
├── Log WHY you are decomposing (mandatory)
│   Choose exactly one reason:
│   - "context_overflow: approaching context limit after N turns"
│   - "multi_phase: task has K independent sub-problems on different files"
│   - "triage_large: instruction >500 words, >3 files, clear sub-problems"
├── Serialize your exploration findings into subtask descriptions
│   (file paths discovered, patterns noticed, test command, edge cases)
├── Create subtasks (max 4, max 1 level deep)
├── Create a verify task that depends on all subtasks
├── Mark seed task done
└── Coordinator dispatches workers
```

### Why Try-First Beats Plan-First

| Metric | Always-Decompose (current G) | Try-First (proposed) |
|--------|------------------------------|----------------------|
| Easy tasks (28%) | 4-8 min overhead, then agent solves in <2 min | Solved directly in <2 min |
| Medium tasks (28%) | 4-8 min overhead, context loss | Solved directly in 5-15 min |
| Borderline tasks (17%) | Overhead + risk of poor decomposition | Solved directly; decompose only if stuck |
| Hard tasks (28%) | Overhead, but decomposition helps | Try first, then decompose with exploration context preserved in subtask descriptions |

The key insight: for the 72% of tasks where decomposition is harmful, try-first costs nothing extra — the agent just solves the task. For the 28% where decomposition helps, the agent's Phase 1 exploration isn't wasted — it gets serialized into higher-quality subtask descriptions.

---

## The Smart Fanout Calculus

### Formal Decision Function

```python
def should_decompose(task, agent_state) -> tuple[bool, str]:
    """
    Returns (decompose: bool, reason: str).
    Called during triage AND during implementation if context pressure detected.
    """

    # ── Triage-time signals (available before any implementation) ──

    instruction_words = count_words(task.instruction)
    files_to_modify = count_referenced_files(task.instruction)
    test_count = count_test_files(task.working_dir)
    has_independent_subproblems = detect_independent_phases(task.instruction)

    # Fast path: small tasks never decompose
    if instruction_words < 300 and files_to_modify <= 2 and test_count <= 5:
        return (False, "fast_path: small task")

    # Large + structured: decompose at triage
    if (instruction_words > 500
        and files_to_modify > 3
        and has_independent_subproblems):
        return (True, f"triage_large: {instruction_words} words, "
                      f"{files_to_modify} files, independent sub-problems")

    # Default: try first
    return (False, "default: attempting direct implementation")


def should_decompose_mid_task(agent_state) -> tuple[bool, str]:
    """
    Called periodically during implementation.
    Detects context pressure that warrants switching to decomposition.
    """

    # Context utilization check
    if agent_state.context_utilization > 0.40:
        remaining_work = estimate_remaining(agent_state)
        if remaining_work > 0.3:  # >30% of work remaining
            return (True, f"context_overflow: {agent_state.context_utilization:.0%} "
                         f"utilized, ~{remaining_work:.0%} work remaining")

    # Stall detection: many turns with no test progress
    if (agent_state.turns > 25
        and agent_state.tests_passing_delta == 0
        and agent_state.turns_since_last_progress > 10):
        return (True, "stall: no progress in 10+ turns")

    return (False, "continuing direct implementation")
```

### Observable Signals (What the Agent Can Actually Measure)

The agent can't compute exact token counts, but it can observe proxy signals:

| Signal | How to Measure | Threshold | What It Means |
|--------|---------------|-----------|---------------|
| **Instruction length** | Count words in task description | < 300 = small, > 500 = large | Correlates with task complexity |
| **File scope** | `ls` + count files mentioned in instruction | ≤ 2 = focused, > 3 = broad | More files = more context consumed |
| **Test count** | `ls tests/` + count test files/functions | ≤ 5 = manageable, > 10 = iterative | More tests = more verify cycles |
| **Independent phases** | Look for: "then", numbered steps, distinct modules | Present = decomposable | Key decomposition trigger |
| **Context pressure** | Re-reading files already read; forgetting earlier edits; tool calls returning truncated content | Any of these = pressure | Switch to decompose mode |
| **Turn count** | Internal counter | > 25 with no test progress | Stall indicator |

### Decomposition Constraints

When decomposition IS triggered, these hard constraints apply:

1. **Max 4 subtasks**: More subtasks = more overhead. 2-3 is ideal.
2. **Max 1 level deep**: Subtasks must NOT create their own subtasks. The meta-prompt explicitly tells workers: "Implement the solution directly. Do NOT create further subtasks."
3. **No shared files**: Each subtask must list its file scope. If two subtasks touch the same file, merge them into one or serialize them with `--after`.
4. **Mandatory verify task**: Every decomposition includes a verify task that depends on all work tasks and runs the test suite.
5. **Context transfer**: Each subtask description must include the architect's exploration findings — file paths, patterns, edge cases, test command.

---

## Updated Meta-Prompt for Condition G

### Replacement: `CONDITION_G_SMART_META_PROMPT`

```
You are solving a programming task. You have two strategies available:

**Strategy 1 — Direct Implementation (default)**
Implement the solution yourself. This is fastest for most tasks.

**Strategy 2 — Decomposition (only when needed)**
Break the task into subtasks and let other agents implement them in parallel.
Only use this if direct implementation won't work.

## Step 1: Triage (spend < 2 minutes here)

Read the task. Scan the working directory (`ls`, `ls tests/`). Then decide:

**Use DIRECT IMPLEMENTATION if ANY of these are true:**
- The instruction is under ~300 words
- You need to modify 2 or fewer files
- The test suite has 5 or fewer tests
- The task is a single logical unit of work (even if complex)
- You're not sure → default to direct implementation

**Use DECOMPOSITION only if ALL of these are true:**
- The instruction is over ~500 words
- You need to modify 3+ distinct files
- The work splits into 2-4 independent sub-problems (different files, no ordering)
- Each sub-problem is substantial enough to benefit from a fresh context window

**Log your decision:**
```bash
wg log {seed_task_id} "FANOUT_DECISION: <direct|decompose> — <reason>"
```

## If Direct Implementation

Implement the solution. Write code, modify files, run tests.

If tests pass → `wg done {seed_task_id}`

If you notice context pressure during implementation (re-reading files you've
already read, losing track of earlier changes, tool outputs getting truncated),
you may switch to decomposition for the REMAINING work. Log:
```bash
wg log {seed_task_id} "FANOUT_SWITCH: direct→decompose — context pressure after N turns"
```
Then create subtasks for the unfinished portions only.

## If Decomposition

1. **Serialize your exploration** — everything you learned during triage goes
   into the subtask descriptions. File paths, test commands, patterns, edge cases.
   Workers only see what you write in `wg add -d "..."`.

2. **Create 2-4 focused subtasks** (NEVER more than 4):
```bash
wg add "Part 1: <specific scope>" -d "## What to do
<concrete instructions>

## Files to modify
- path/to/file1.py

## How to verify
Run: <test command>

## IMPORTANT
Implement directly. Do NOT create subtasks. Do NOT decompose further."
```

3. **Wire in a verify task**:
```bash
wg add "Verify: run full test suite" --after part-1,part-2 \
  -d "Run the test suite: <test command>.
If ALL tests pass: wg done <your-task-id> --converged
If tests fail: wg log <your-task-id> 'what failed' then wg done <your-task-id>"
```

4. **Create the retry loop** (if the task warrants iteration):
```bash
wg edit part-1 --add-after verify --max-iterations 3
```

5. **Explicitly release the wired component**:
```bash
wg publish part-1 --wcc
```

6. **Mark your seed task done**:
```bash
wg done {seed_task_id}
```

## Hard constraints
- NEVER create more than 4 subtasks
- Subtasks must NOT create their own subtasks (1 level max)
- If two subtasks would modify the same file, merge them or serialize with --after
- Always include a verify task at the end
```

### Key Differences from Current Meta-Prompt

| Aspect | Current G Prompt | Smart Fanout Prompt |
|--------|-----------------|---------------------|
| Default mode | "DO NOT write code" — always decompose | "Implement yourself" — decompose only when needed |
| Decision guidance | None — agent always decomposes | Explicit triage criteria with measurable thresholds |
| Mid-task switching | Not possible — agent commits to decompose at start | Can switch from direct→decompose if context pressure detected |
| Depth limit | No limit — workers can re-decompose | Explicit: "Do NOT create subtasks" in worker descriptions |
| Subtask count | No limit | Hard cap at 4 |
| Context transfer | "Put ALL necessary context" (vague) | Structured: exploration findings, file paths, test commands, edge cases |
| Audit trail | None | Mandatory `wg log` for decision + reason |
| Worker instructions | Generic | Include "Do NOT decompose further" |

---

## Integration into adapter.py

### Config Changes

```python
# In CONDITION_CONFIGS:
"G": {
    "exec_mode": "full",
    "context_scope": "graph",
    "agency": None,
    "exclude_wg_tools": False,
    "max_agents": 4,                    # Reduced from 8 — most trials use 1-2 agents
    "autopoietic": True,
    "smart_fanout": True,               # NEW: use smart meta-prompt
    "coordinator_agent": True,
    "heartbeat_interval": 30,
    "worktree_isolation": True,         # CHANGED: prevent file conflicts between agents
},
```

### Changes Required

1. **New constant**: `CONDITION_G_SMART_META_PROMPT` (the prompt above)
2. **Prompt selection**: When `smart_fanout=True`, use the smart prompt instead of `CONDITION_G_META_PROMPT`
3. **Config change**: `max_agents=4` (down from 8), `worktree_isolation=true`
4. **Worker template**: Add "Do NOT decompose further" boilerplate to worker task descriptions (can be done in the meta-prompt instructions)

### Local Runner Changes (run_qwen3_hard_20_g.py)

1. Replace `CONDITION_G_META_PROMPT` with the smart version
2. Set `worktree_isolation = true` in `write_trial_config()`
3. Reduce `max_agents` from 4 to 3 (local hardware constraint — 4 concurrent 30B model instances is marginal)

---

## Validation Plan: Before/After Comparison

### Experiment Design

**Hypothesis:** Smart fanout will achieve ≥80% pass rate on the 18-task qwen3 benchmark (vs 72% for Condition A and ~61% estimated for current G).

**Variables:**
- Independent: meta-prompt (current G vs smart fanout)
- Controlled: model (qwen3-coder-30b), hardware (local SGLang), task set (18 tasks), trial timeout (30 min)
- Dependent: pass rate, time-to-pass, decomposition rate, agent count per trial

### Comparison Conditions

| Condition | Label | What's Tested |
|-----------|-------|---------------|
| A (existing data) | `qwen3-hard-20-a` | Single agent, no wg tools, no decomposition |
| G-old (need to run) | `qwen3-hard-20-g-old` | Current "always decompose" meta-prompt |
| G-smart (need to run) | `qwen3-hard-20-g-smart` | Smart fanout meta-prompt |

### Metrics to Collect

| Metric | Source | What It Tells Us |
|--------|--------|-----------------|
| **Pass rate** (primary) | Test suite result | Overall effectiveness |
| **Decomposition rate** | Count trials with `FANOUT_DECISION: decompose` in logs | How selective the agent is |
| **Correct decomposition rate** | Cross-reference: did decomposed tasks actually need it? | Quality of the decision function |
| **Time-to-pass** | Trial elapsed time for passing trials | Overhead impact |
| **Agent count per trial** | `wg agents` output captured in metrics | Parallelism utilization |
| **Context switch count** | Count `FANOUT_SWITCH` log entries | Mid-task adaptation |
| **Per-task-class pass rates** | Break down by easy/medium/borderline/hard | Confirm no regression on easy tasks |

### Success Criteria

The smart fanout design is validated if:

1. **No regression on easy/medium tasks**: Tasks that Condition A passes must still pass under G-smart (≥12/13 on the A-passing set)
2. **Improvement on hard tasks**: G-smart passes ≥2 of the 5 tasks that A fails on (context overflow set)
3. **Selective decomposition**: Agent decomposes ≤35% of tasks (only the genuinely complex ones)
4. **Correct decomposition**: ≥80% of decomposition decisions are for tasks that would benefit (the hard/borderline set)
5. **Overall pass rate ≥ 80%**: Combined across all 18 tasks

### Collection Script Additions

The `FANOUT_DECISION` and `FANOUT_SWITCH` log lines need to be captured in the results collection. Add to `collect_condition_g_results.py`:

```python
# In the per-trial analysis:
for log_line in trial_logs:
    if "FANOUT_DECISION:" in log_line:
        trial_result["fanout_decision"] = log_line.split("FANOUT_DECISION:")[1].strip()
    if "FANOUT_SWITCH:" in log_line:
        trial_result["fanout_switches"] = trial_result.get("fanout_switches", [])
        trial_result["fanout_switches"].append(log_line.split("FANOUT_SWITCH:")[1].strip())
```

### Expected Outcomes by Task Class

| Task Class | Count | A Pass | G-smart Expected | Mechanism |
|------------|-------|--------|------------------|-----------|
| Easy | 5 | 5/5 | 5/5 | Direct implementation, no decomposition |
| Medium | 5 | 5/5 | 5/5 | Direct implementation, no decomposition |
| Borderline | 3 | 3/3 | 3/3 | Direct implementation; may switch to decompose if context pressure |
| Hard (ctx overflow) | 5 | 0/5 | 2-4/5 | Triage→decompose (>500 words, >3 files) or try→switch→decompose |
| **Total** | **18** | **13/18** | **15-17/18** | |

---

## Risk Analysis

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| Agent ignores triage criteria, always implements directly | Medium | Misses decomposition opportunities on hard tasks | Make triage explicit with `wg log` — auditable |
| Agent over-decomposes despite guidance | Low | Same overhead problem as current G | Hard cap of 4 subtasks + "default to direct" framing |
| Smart prompt is too long, consumes context budget | Low | Less room for actual work | Smart prompt is ~400 words vs ~300 for current; marginal increase |
| Mid-task switching creates partially-done state | Medium | Subtasks duplicate work already done | Instruct agent to create subtasks only for *remaining* work |
| Workers decompose despite prohibition | Low | Cascading depth | "Do NOT create subtasks" in every worker description |

---

## Implementation Roadmap

1. **Write the smart meta-prompt** (this document provides the full text)
2. **Update adapter.py**: Add `CONDITION_G_SMART_META_PROMPT`, wire `smart_fanout` config flag
3. **Update local runner**: Replace meta-prompt, enable worktree isolation
4. **Run smoke test**: 3 tasks (1 easy, 1 medium, 1 hard) to verify the agent follows the triage flow
5. **Run full comparison**: 18 tasks × G-smart, collect FANOUT_DECISION metrics
6. **Analyze**: Compare against A baseline and (if available) G-old results

---

## Appendix: Task Classification Reference

For the 18-task qwen3 benchmark, the expected triage decisions:

| Task | Words | Files | Tests | Expected Decision | Reason |
|------|-------|-------|-------|-------------------|--------|
| text-processing | ~100 | 1 | 2 | Direct | Fast path |
| shell-scripting | ~80 | 1 | 2 | Direct | Fast path |
| algorithm | ~120 | 1 | 3 | Direct | Fast path |
| data-processing | ~100 | 1 | 3 | Direct | Fast path |
| debugging | ~150 | 1 | 3 | Direct | Fast path |
| file-ops | ~200 | 2 | 4 | Direct | Fast path |
| sysadmin | ~250 | 2 | 4 | Direct | Default |
| ml | ~300 | 2 | 5 | Direct | Default |
| build-cython-ext | ~350 | 3 | 4 | Direct | Default (borderline but ≤3 files) |
| fix-code-vulnerability | ~300 | 2 | 5 | Direct | Default |
| multi-module-type-migration | ~400 | 3 | 6 | Direct | Default (few independent sub-problems) |
| mailman | ~450 | 3 | 5 | Direct | Default |
| configure-git-webserver | ~500 | 3 | 6 | Direct | Default (sequential, not independent) |
| cobol-modernization | ~600 | 4 | 8 | **Decompose** | Large + multi-file + independent phases |
| multi-source-data-merger | ~550 | 4 | 7 | **Decompose** | 3 formats → merge → conflicts = independent |
| financial-document-processor | ~600 | 4 | 8 | **Decompose** | classify → extract → summarize = independent |
| constraints-scheduling | ~500 | 3 | 8 | Direct (borderline) | 3 files but tightly coupled |
| iterative-test-fix | ~550 | 4 | 15 | **Decompose** | 6 bugs × 15 tests, bugs are independent |

Expected decomposition rate: 4/18 = 22% (within the ≤35% target).
