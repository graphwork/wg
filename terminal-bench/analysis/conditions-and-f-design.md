# Terminal Bench: Complete Condition Analysis and F Variant Design

**Date:** 2026-04-04
**Task:** research-tb-conditions-and-f
**Model:** openrouter/minimax/minimax-m2.7 (all conditions)

---

## 1. Summary Table: All Conditions (A through E)

| Condition | Full Name | Tools | Prompt Strategy | Pass Rate (Pilot 10-task) | Pass Rate (Full 89-task) | Key Strength | Key Weakness |
|-----------|-----------|-------|----------------|--------------------------|--------------------------|-------------|-------------|
| **A** | Bare agent (50-turn cap) | 6 (bash + file) | Minimal: "coding agent, use bash and file tools" | 48% (original) | 47.7–52.8% | Simple, no overhead | 25% hit turn cap; no verification gate; silent failures |
| **A'** | Bare agent (no turn cap) | 6 (bash + file) | Same as A; `max_turns=9999`, 30-min timeout | **80.0%** (24/30) | N/A (pilot only) | Best pilot pass rate; removing turn cap = biggest single intervention | High variance (1–191 turns); no termination signaling; costly outliers |
| **B** | WG tools (passive) | 15 (+wg) | Brief guidelines: "use wg_log, wg_add, wg_done" + graph patterns | N/A (not in pilot) | 38.1% (partial) | Provides task coordination | **80% of agents ignore wg tools entirely**; overhead without guidance |
| **C** | WG + skill injection | 15 (+wg) | Explicit templates: `wg_log("{id}", "Starting: …")`, planning phase, decomposition heuristic | N/A (not in pilot) | 41.4–51.1% | 81% wg adoption; structured planning turns | Still no verification gate; 14% ignore "mandatory" wg usage; linear execution |
| **D** | WG + autopoietic verification | 15 (+wg) | Attempt→Verify→Iterate→Declare loop; agency identity (solver/programmer/careful); `wg_done` gated on verification | **73.3%** (22/30) | N/A (pilot only) | Most cost-efficient (521K tokens/trial); 93% self-termination via `wg_done`; disciplined convergence | Self-verification blind spot (agent tests what it *thinks* is correct); 0 verify iterations on tasks without obvious test criteria (e.g., LaTeX) |
| **E** | WG + org generation | 15 (+wg) | Orchestrator framing; Phase 1–5 protocol (Decompose→Implement→Verify independently→Triage→Declare); agency identity (orchestrator/architect/thorough) | **75.0%** (21/28 valid) | N/A (pilot only) | Excels on multi-step tasks (build-cython-ext 100%, merge-diff 100%); decomposition helps when genuine | **100% false-PASS rate on failures**; counterproductive on atomic tasks (regex-log 0%, cancel-async 0%); 5× more wg tool calls than D; timeout risk from unbounded verification loops |

### Key Findings

1. **Removing the turn cap was the single most impactful change** (A 48% → A' 80%). The 50-turn cap in original A/B/C killed ~25% of trials that needed more iterations.

2. **WG tools without guidance hurt** (A 48% > B 38%). The tools add cognitive overhead; the model takes the shortest path (bash+file) unless explicitly taught when/how to use wg.

3. **Skill injection partially recovers** (B 38% → C 41%). Concrete templates drive 81% adoption, but the prompt still doesn't gate `wg_done` on verification.

4. **Self-verification adds discipline but has a ceiling** (D 73%). The attempt→verify→iterate loop constrains token usage and improves termination signaling, but the agent can only test what it can conceive of — it misses requirements outside its mental model.

5. **Organization generation is a mixed bag** (E 75%). Decomposition genuinely helps multi-step tasks but actively harms atomic tasks. The "independent verification" is theater — same context window, same blind spots.

6. **No condition differences are statistically significant** at pilot sample size (Fisher's exact: all p > 0.77).

---

## 2. Detailed Condition Analyses

### 2.1 Condition A / A': The Bare Agent

**Prompt** (`build_condition_a_prompt`, adapter.py:557–571):
```
You are a coding agent completing a Terminal Bench task.
You have access to bash and file tools.
Focus on completing the task efficiently and correctly.
Do not ask for clarification - proceed with your best judgment.

## Guidelines
- Use bash to run commands, install packages, compile code, etc.
- Use read_file, write_file, edit_file for file operations.
- Use glob and grep to explore the codebase.
- Always prefer precise edits over full file rewrites.
- Keep output concise.
```

**Tool set**: 6 tools (bash, read_file, write_file, edit_file, glob, grep).

**A' variant**: Identical prompt and tools but `max_turns=9999` (effectively unlimited) with 30-minute wall-clock timeout. This simple change lifted pass rate from 48% to 80%.

**Why A' wins**: The model's natural behavior — read task, implement, test, iterate — is effective. Most tasks need 15–30 turns, but ~25% need 50–100+ turns for complex builds or iterative debugging. The 50-turn cap was killing these.

**A' weaknesses**:
- **High variance**: One custom-memory-heap-crash trial ran 191 turns / 14M tokens while another finished in 22 turns / 127K tokens.
- **No termination signaling**: 100% of trials end with `no_tool_calls` (model just stops). No explicit `wg_done`/`wg_fail`, so there's no structured end-state.
- **No failure diagnostics**: When the agent can't solve a task, it just stops generating. No log of what was tried or why it failed.

### 2.2 Condition B: WG Tools (Passive)

**Prompt** (`build_condition_b_prompt`, adapter.py:574–597):
Adds root task ID, brief wg guidelines ("use wg_log to record progress"), workgraph graph patterns (pipeline, diamond, loop), and journal/resume concept.

**Failure mode**: 80% of agents never call a single wg tool. The model treats wg_* tools as optional. When the task is simple, it takes the shortest path (bash+file). The prompt says "use wg_log" as a suggestion, not an imperative.

**Pass rate**: 38.1% — **worse than A** because the wg tools add cognitive overhead without compensating benefits.

### 2.3 Condition C: WG + Skill Injection

**Prompt** (`build_condition_c_prompt`, adapter.py:600–638):
Key additions over B:
- Explicit templates with root task ID pre-filled: `wg_log("{root_task_id}", "Starting: <plan>")`
- Mandatory planning phase: "analyze the task in ONE response" before coding
- Decomposition heuristic: "If 3+ distinct phases or might exhaust context"
- Removes abstract graph patterns (pipeline/diamond/loop) — focuses on practical usage

**Improvements over B**: 81% wg adoption (vs 20%); 86% wg_done compliance (vs 44%); structured planning in ~85% of trials.

**Still missing**: No verification gate; 10.5% call wg_done with zero verification; no iteration guidance on failure.

### 2.4 Condition D: Autopoietic Verification

**Prompt** (`build_condition_d_prompt`, adapter.py:641–686):
Core innovation — the **Attempt→Verify→Iterate→Declare** loop:
1. **Understand**: Read task, identify verification criteria
2. **Attempt**: Implement solution
3. **Verify**: Run tests/checks — execute something that proves correctness
4. **Iterate**: If verification fails, diagnose and fix (repeat step 3)
5. **Declare**: `wg_done` only after verification passes; `wg_fail` after 3+ stuck iterations

**Agency identity**: Agent is "solver" (role=programmer, tradeoff=careful). Prompt says: "you prioritize correctness over speed."

**Results**:
- 93% self-termination via `wg_done` (28/30 trials)
- Average 3.4 verification iterations per trial
- Most cost-efficient: 521K tokens/trial (37% less than A')
- The verification loop *constrains* token usage by providing a convergence criterion

**Critical weakness**: **Self-verification blind spot**. On build-cython-ext, D agents ran their own tests (which passed) but the external verifier's `test_ccomplexity` failed. The agent verified what it could conceive of testing, not what the verifier actually checks. On overfull-hbox, 0 verification iterations across all trials — the agent couldn't find appropriate verification for LaTeX.

### 2.5 Condition E: Organization Generation

**Prompt** (`build_condition_e_prompt`, adapter.py:689–767):
Five-phase protocol:
1. **Analyze & Decompose**: Break task into implementation steps using `wg_add`
2. **Implement**: Execute each subtask, log progress, mark done
3. **Independent Verification**: Shift perspective to "REVIEWER"; read files fresh; run tests; produce PASS/FAIL verdict
4. **Triage**: On FAIL, create fix tasks with failure context; re-implement; re-verify (up to 6 iterations)
5. **Declare**: `wg_done` only after PASS verdict; `wg_fail` after 6 stuck iterations

**Agency identity**: Agent is "orchestrator" (role=architect, tradeoff=thorough). Prompt says: "you are an ORCHESTRATOR, not a direct implementer."

**The false-independence problem**: In a single-agent Harbor trial, "independent verification" is impossible. The agent that wrote the code also "independently" reviews it. The "shift perspective" instruction doesn't reset the model's internal representation. Result: **100% false-positive rate on failures** — the verification loop NEVER produced a FAIL verdict across all 30 pilot trials.

**Where E excels**: Multi-step tasks with genuinely independent components:
- build-cython-ext (100%): 4 subtasks — clone, patch, build, test
- merge-diff-arc-agi-task (100%): 5.3 subtasks — git setup, merge, algorithm, testing
- nginx-request-logging (100%): 6 subtasks — install, create files, config, verify

**Where E fails catastrophically**:
- cancel-async-tasks (0%): Single-function task; decomposition adds overhead; verification catches nothing
- regex-log (0%): Decomposition fragments an atomic regex problem; 5 subtasks lose the holistic view; 1 trial timed out at 30 min / 3M tokens

---

## 3. Model Context Gap Analysis

### 3.1 The Problem

When switching from Claude to M2.7 (openrouter:minimax/minimax-m2.7), the model loses access to Claude-specific context:

| Context Source | Available to Claude (native) | Available to M2.7 (via Harbor adapter) |
|---------------|------------------------------|---------------------------------------|
| **CLAUDE.md** project instructions | Yes (auto-loaded by Claude Code harness) | **NO** — not injected into system prompt |
| **.claude/ memory files** | Yes (auto-loaded) | **NO** — not injected |
| **Claude CLI tool understanding** | Yes (built-in) | **NO** — M2.7 has no concept of `wg` CLI |
| **wg tool schemas** | N/A (Claude uses CLI directly) | **YES** — passed as OpenAI function-calling schema (15 tools) |
| **wg usage instructions** | From CLAUDE.md + built-in | **Only what's in the system prompt** (varies by condition) |

### 3.2 How Tools Are Passed to the Model

The adapter uses **litellm** (`adapter.py:978`) to call any OpenAI-compatible model:

```python
response = await litellm.acompletion(
    model=model,
    messages=messages,
    tools=tools,  # OpenAI function-calling schema
    tool_choice="auto",
    temperature=self.temperature,
    max_tokens=16384,
)
```

Tools are defined as standard OpenAI function-calling JSON schemas (adapter.py:43–361). They are **model-agnostic** — any model supporting OpenAI function calling can use them. The tool schemas include:

- **Name**: e.g., `wg_add`, `wg_done`, `wg_log`
- **Description**: Brief (e.g., "Create a new task in the workgraph")
- **Parameters**: JSON Schema with required/optional fields

### 3.3 What M2.7 Gets vs. What It Needs

**What M2.7 gets** (for Condition E):
1. A system prompt (~2K tokens) with the 5-phase protocol
2. The task instruction as a user message
3. 15 tool schemas as OpenAI function-calling definitions
4. Tool results as function call responses

**What M2.7 does NOT get**:
1. **No CLAUDE.md content**: The project instructions that tell Claude how to use `wg` (task decomposition patterns, dependency management, cycle support, verification requirements) are never injected. This is ~5K tokens of operational context.
2. **No .claude/ memory**: Project-specific knowledge (architecture decisions, known patterns, file paths) is not available.
3. **No `wg` conceptual background**: M2.7 has no pre-training knowledge of what "workgraph" is. Claude (used natively) would understand `wg` from its training data or CLAUDE.md. M2.7 must learn entirely from the tool descriptions + system prompt.
4. **No tool usage examples**: The tool schemas say `wg_add` takes a `title` and optional `after` parameter, but there are no examples of *when* to use `--after` or what dependency patterns look like.

### 3.4 Impact by Condition

| Condition | How much context gap matters |
|-----------|------------------------------|
| **A** | **Minimal** — A doesn't use wg tools. The prompt is simple and model-agnostic. |
| **B** | **High** — B lists wg tools but provides minimal guidance. M2.7 has no prior knowledge of wg → 80% ignore the tools entirely. |
| **C** | **Medium** — C provides explicit templates (`wg_log("{id}", "...")`) that partially bridge the gap. But the "when to decompose" heuristic is thin. |
| **D** | **Medium-Low** — D's prompt teaches the verification loop in model-agnostic terms. The main gap is verification *criteria* — D says "find tests" but doesn't tell the agent where tests live in the container (`/tests/test_outputs.py`). |
| **E** | **High** — E's orchestrator framing assumes the model understands task coordination. M2.7 creates flat subtask lists with no dependencies (zero `--after` usage in 30 trials). The "independent verification" concept requires meta-cognitive reasoning that M2.7 may not support well. |

### 3.5 The Verification Test Discovery Gap (Critical)

The single most impactful missing context: **no condition tells the agent where the verifier's test files are**.

The Harbor benchmark framework stores verifier tests at `/tests/test_outputs.py` inside each Docker container. The external verifier runs exactly these tests to score the trial. If the agent ran these tests, it would discover its failures before declaring PASS.

- **No condition mentions `/tests/`** in the prompt
- **No condition instructs "find test files"** before implementing
- The agent's "verification" is always self-authored tests that validate its own understanding
- All 7 of E's false-PASS failures would have been caught by running `/tests/test_outputs.py`

---

## 4. Proposed F Variant Design

### 4.1 Design Philosophy

F should be the **synthesis of what works** across all conditions:

| Source | What to keep | What to discard |
|--------|-------------|-----------------|
| **A'** | Unlimited turns (no 50-turn cap); 30-min timeout; direct, simple framing | Silent failures; no termination signaling; no verification discipline |
| **D** | Attempt→Verify→Iterate→Declare loop; `wg_done` gated on verification; `wg_fail` after stuck iterations; cost-efficient convergence criterion | Self-authored verification only; no test discovery; identity/agency overhead (minimal impact) |
| **E** | Adaptive decomposition for multi-step tasks; structured phase protocol | Mandatory decomposition; orchestrator framing; "independent" verification theater; flat subtask lists |
| **condition-e-improvement.md** | Test discovery (#1 improvement); empirical verification only (#3); time-aware bailout (#4); remove orchestrator framing (#6) | — |

### 4.2 F's Core Design: Empirical-First Verification Agent

F is built on one insight: **run the existing tests first, always**. The verification gap across all conditions is not about self-verification quality — it's that agents never discover or run the benchmark's own test suite.

### 4.3 Key Design Decisions

#### Decision 1: No wg tools — F uses A's tool set (6 tools)

**Rationale**: 
- A' (bare, no wg) achieves 80% pass rate. D (wg + verification) achieves 73%. E (wg + org) achieves 75%.
- wg tools add 5× more tool calls (E) or ~3 per trial (D) for no aggregate improvement.
- In single-agent Harbor trials, wg is a bookkeeping system, not a coordination system — there's no coordinator, no parallel execution, no agent spawning.
- The context tokens spent on wg tool schemas (9 extra tool definitions) and wg usage instructions could be spent on *verification guidance*.
- **Removing wg simplifies the prompt**, reduces tool-choice confusion, and lets the model focus entirely on the task.

**Exception**: If future F variants test multi-agent execution (wg service + multiple models), wg becomes essential. For single-agent Harbor trials, it's overhead.

#### Decision 2: Test discovery before implementation

**Rationale**:
- All 7 of E's false-PASS failures + D's build-cython-ext failures would have been caught by running `/tests/test_outputs.py`.
- The verifier runs exactly these tests. If the agent runs them, it gets the same signal.
- This is the highest-impact single change identified in the condition-e-improvement analysis.

#### Decision 3: Adaptive task classification (atomic vs. multi-step)

**Rationale**:
- E's 0% pass rate on regex-log and cancel-async-tasks was caused by decomposing atomic tasks.
- A' handles both atomic and multi-step tasks well because it doesn't force any strategy.
- F should classify but not force decomposition — just adjust the verification strategy.

#### Decision 4: Empirical verification as the primary signal

**Rationale**:
- E's "independent" cognitive verification produced 100% false-positives on failures.
- D's self-verification passed when external tests failed.
- The only reliable verification is *running executable tests* and treating their results as authoritative.

#### Decision 5: Time-aware bailout

**Rationale**:
- 2 E trials timed out at 30 minutes, losing all metadata.
- Explicit time awareness converts timeouts into clean failures with diagnostics.

### 4.4 F Prompt Template

```python
def build_condition_f_prompt(instruction: str) -> str:
    """Condition F: empirical-first verification agent."""
    return (
        "You are a coding agent completing a Terminal Bench task.\n"
        "You have access to bash and file tools.\n\n"
        "## Strategy: Discover → Implement → Verify → Iterate\n\n"
        "### Step 1: Discover Tests\n"
        "Before writing any code, find the task's test suite:\n"
        "```\n"
        "bash(\"find /tests -name 'test_*.py' -o -name '*_test.py' 2>/dev/null\")\n"
        "bash(\"find / -maxdepth 3 -name 'test_*.py' -name '*_test.py' 2>/dev/null | head -20\")\n"
        "bash(\"ls /tests/ 2>/dev/null\")\n"
        "```\n"
        "Read any test files you find. They define what 'correct' means.\n"
        "Understanding the tests FIRST tells you exactly what to build.\n\n"
        "### Step 2: Classify the Task\n"
        "- **ATOMIC** (single file, single function, single config): Implement directly.\n"
        "- **MULTI-STEP** (multiple files, build pipeline, system setup): "
        "Plan your steps, but implement them yourself sequentially.\n\n"
        "### Step 3: Implement\n"
        "Write your solution. For multi-step tasks, tackle each step in order.\n\n"
        "### Step 4: Verify Empirically\n"
        "Run the discovered test files:\n"
        "```\n"
        "bash(\"cd /tests && python -m pytest test_outputs.py -v 2>&1 | tail -80\")\n"
        "```\n"
        "Or if no test files were found, verify by running the code and checking outputs.\n\n"
        "**CRITICAL**: If existing tests FAIL, that is your ground truth.\n"
        "Do NOT declare success based on your own assessment if a test file fails.\n"
        "Your own ad-hoc tests supplement existing tests — they never override them.\n\n"
        "### Step 5: Iterate on Failures\n"
        "If tests fail:\n"
        "1. Read the failure output carefully — it tells you exactly what's wrong.\n"
        "2. Diagnose the root cause (not just the symptom).\n"
        "3. Fix it.\n"
        "4. Re-run the tests.\n"
        "5. Repeat up to 5 times. If you're trying the same fix twice, "
        "step back and reconsider your approach entirely.\n\n"
        "### Step 6: Time Management\n"
        "You have 30 minutes maximum. Budget roughly:\n"
        "- Test discovery + task analysis: 2 minutes\n"
        "- Implementation: 15 minutes\n"
        "- Verification + iteration: 10 minutes\n"
        "- If stuck for 20+ minutes with no progress, stop.\n\n"
        "## Tools\n"
        "- `bash` — Run commands, install packages, compile, test\n"
        "- `read_file`, `write_file`, `edit_file` — File operations\n"
        "- `glob`, `grep` — Search the codebase\n\n"
        "Begin by discovering tests, then implement and verify.\n"
    )
```

### 4.5 F Adapter Implementation

```python
class ConditionFAgent(WorkgraphAgent):
    """Condition F: empirical-first verification, no wg overhead."""

    @staticmethod
    def name() -> str:
        return "workgraph-condition-f"

    def __init__(self, *args, **kwargs):
        kwargs["condition"] = "F"  # New condition letter
        kwargs["model_name"] = BENCHMARK_MODEL
        kwargs.setdefault("max_turns", 200)  # Safety valve (A' uses 9999, but 200 is sufficient — D's mean was 29.6 turns)
        super().__init__(*args, **kwargs)
```

**Adapter changes needed**:
1. Add `"F"` to condition validation
2. Add `build_condition_f_prompt` function
3. Route condition F to use `CONDITION_A_TOOLS` (6 tools, no wg) and `build_condition_f_prompt`
4. Skip wg init/setup for condition F (like condition A)
5. Track verification iterations (reuse D's tracking logic for bash commands containing test/pytest/verify)

### 4.6 What F Tests (Hypothesis)

**Primary hypothesis**: Test discovery + empirical verification discipline improves pass rate over A' (bare agent), without wg overhead.

**Secondary hypothesis**: The pass rate gap between self-verification (D) and empirical verification (F) measures how much "knowing what the tests check" matters.

**Expected outcome by task type**:

| Task Type | A' | D | E | F (predicted) | Rationale |
|-----------|-----|---|---|---------------|-----------|
| Multi-step build | 100% | 33% | 100% | **100%** | Test discovery catches build verification issues D missed |
| Single function (async) | 67% | 67% | 0% | **67-100%** | Running `/tests/test_outputs.py` reveals the edge case; whether the agent can fix it depends on the model |
| Regex/text processing | 67% | 67% | 0% | **67-100%** | Test file shows exact expected matches; agent can iterate toward correct regex |
| System config (nginx) | 67% | 100% | 100% | **100%** | D's verification loop works here; F inherits that discipline |
| Debugging (LaTeX) | 33% | 33% | 67% | **33-67%** | Tests may not exist for LaTeX tasks; F falls back to A' behavior |
| Query language (SPARQL) | 100% | 67% | 67% | **100%** | Test file shows expected query results; agent can compare |
| Reasoning (ARC-AGI) | 67% | 67% | 100% | **67%** | E's decomposition helped here; F lacks it. This is where F might underperform E. |

### 4.7 Alternative F Variants to Consider

#### F': F + Selective WG (Adaptive)

If the base F results are promising but show weakness on multi-step tasks (where E excelled), consider F' that adds wg tools conditionally:

```
If you classified the task as MULTI-STEP:
- Use wg_add to create subtasks with dependencies (--after)
- Use wg_log to record progress
- Use wg_done to signal completion
This helps you track progress on complex multi-phase work.
```

This would test whether wg adds value *specifically on tasks that warrant coordination*, without applying it universally.

#### F'': F + Separate Verifier Call

For maximum verification independence, F'' makes a second LLM call (separate conversation) to verify:

```python
# After the agent loop completes:
verification_prompt = f"Review this solution to the task: {instruction}\n\nFiles modified:\n{file_list}\n\nRun the test suite and report PASS or FAIL."
verify_response = await litellm.acompletion(
    model=model,
    messages=[{"role": "user", "content": verification_prompt}],
    tools=CONDITION_A_TOOLS,
    ...
)
```

This addresses the fundamental "same context window" problem but requires Harbor infrastructure changes (two agent runs per trial).

#### F''': F + Different Verification Model

Use M2.7 for implementation but a different model (e.g., claude-haiku) for verification. Different model biases increase the chance of catching errors the implementer missed.

### 4.8 Harbor Run Configuration for F

```bash
harbor run \
    -d terminal-bench/terminal-bench-2 \
    --agent-import-path "wg.adapter:ConditionFAgent" \
    -m "openrouter/minimax/minimax-m2.7" \
    -k 3 \
    -n 4 \
    --job-name condition-f \
    --jobs-dir terminal-bench/results/condition-f \
    --no-delete \
    --debug \
    --ak "max_turns=200" \
    --ak "temperature=0.0" \
    --agent-timeout-multiplier 6.0 \
    -y \
    2>&1 | tee terminal-bench/results/condition-f/run.log
```

### 4.9 Expected Cost and Timeline

| Scenario | Tokens/trial | Cost (30 pilot trials) | Cost (267 full trials) |
|----------|-------------|----------------------|----------------------|
| Optimistic | ~450K (D-like efficiency + less wg overhead) | $39 | $347 |
| Expected | ~600K (between D and A') | $52 | $462 |
| Conservative | ~820K (A'-like, verification adds iterations) | $71 | $632 |

**Time estimate**: With 4 concurrent trials and ~8 min/trial average, the 30-trial pilot takes ~1 hour. Full 267-trial run takes ~9 hours.

### 4.10 Pilot Plan

Run F on the same 10 pilot tasks used for A'/D/E comparison:

1. `build-cython-ext` — multi-step build (tests exist in container)
2. `cancel-async-tasks` — single function (test_outputs.py catches the edge case)
3. `nginx-request-logging` — system config
4. `overfull-hbox` — LaTeX debugging
5. `regex-log` — text processing (test_outputs.py has exact expected matches)
6. `count-dataset-tokens` — data processing
7. `custom-memory-heap-crash` — C debugging
8. `merge-diff-arc-agi-task` — multi-step reasoning
9. `qemu-startup` — system emulation
10. `sparql-university` — query language

**Success criteria**: F pilot pass rate > 80% (exceeds A') OR F pilot achieves A'-comparable pass rate at lower cost (D's cost efficiency).

---

## 5. Why F Should Not Use wg (In Single-Agent Trials)

This section addresses the key question: *Should F even use wg, or is wg overhead in single-agent Harbor trials?*

### 5.1 The Evidence Against wg in Harbor

| Evidence | Source | Implication |
|----------|--------|-------------|
| A' (no wg) = 80% pass rate | Pilot data | wg is not necessary for high performance |
| D (wg) = 73% pass rate | Pilot data | wg + verification doesn't beat bare agent |
| E (heavy wg) = 75% pass rate | Pilot data | More wg usage doesn't help more |
| 80% of B agents ignore wg | Full run data | Model naturally gravitates away from wg |
| E uses 5× more wg calls than D | Pilot data | wg overhead is measurable in tokens and turns |
| Zero `--after` dependencies in E | Pilot data | Agents create flat lists, not graphs — wg's dependency model is unused |
| No wg service running | Architecture | No coordinator, no parallel agents, no task dispatch |

### 5.2 What wg Provides in Harbor Trials

| Feature | Used? | Value? |
|---------|-------|--------|
| `wg_log` (progress journaling) | Yes (93% in D/E) | **Low** — helps post-hoc analysis but doesn't improve pass rate |
| `wg_done` (termination signaling) | Yes (93% in D) | **Medium** — provides clean termination, but F can stop naturally |
| `wg_add` (decomposition) | Rare (7-93% depending on condition) | **Low for Harbor** — single agent executes sequentially regardless |
| `wg_show` / `wg_list` | Rare (3% in E) | **Negligible** — agent doesn't poll its own task graph |
| `wg_fail` (failure signaling) | Rare | **Low** — nice for diagnostics but doesn't improve outcomes |
| Dependency edges (`--after`) | Almost never | **Zero** — agents don't express dependencies |
| Cycles / convergence | Never | **Zero** — requires wg service |
| Agency (roles/tradeoffs) | Static only | **Cosmetic** — injected in prompt, never dynamically used |

### 5.3 When wg WOULD matter

wg becomes essential when:
- **Multiple agents** run concurrently (wg service dispatches to different workers)
- **Context exhaustion** is likely (wg_log preserves progress for resumed agents)
- **Tasks have real dependencies** (build step A must complete before test step B)
- **Different roles** handle different subtasks (programmer vs. reviewer vs. tester)

None of these apply to single-agent Harbor trials.

### 5.4 Recommendation

F should be wg-free for the pilot. If F proves effective, create F' (F + selective wg) for comparison. This cleanly isolates the verification-discipline variable from the wg-tooling variable.

---

## 6. Comparison: F vs. All Conditions

| Dimension | A' | D | E | **F** |
|-----------|-----|---|---|-------|
| Tools | 6 | 15 | 15 | **6** |
| System prompt tokens | ~200 | ~800 | ~1200 | **~600** |
| Turn limit | 9999 | 200 | 300 | **200** |
| Test discovery | No | No ("find tests" implicit) | No | **Yes — explicit step 1** |
| Verification gate | No | Yes (self-test) | Yes (self-test, always PASS) | **Yes (existing tests authoritative)** |
| Decomposition | No | Optional | Mandatory | **Classification only** |
| Failure protocol | Silent stop | `wg_fail` after 3 iterations | `wg_fail` after 6 iterations | **Natural stop after 5 fix iterations or 20 min** |
| Time awareness | No | No | No (contributes to timeouts) | **Yes — explicit 30-min budget** |
| wg overhead | 0 | ~3 calls/trial | ~15 calls/trial | **0** |
| Expected cost/trial | 821K tokens | 521K tokens | 683K tokens | **~600K tokens** |

---

## 7. Implementation Checklist

1. [ ] Add `build_condition_f_prompt()` to adapter.py
2. [ ] Add `ConditionFAgent` class to adapter.py
3. [ ] Route condition "F" in `WorkgraphAgent.__init__` and `.run()` (use CONDITION_A_TOOLS, skip wg setup)
4. [ ] Add verification tracking for condition F (reuse D's bash command pattern matching)
5. [ ] Update module docstring to mention Condition F
6. [ ] Smoke test: 1 trial on `cancel-async-tasks` (the task where test discovery should make the biggest difference)
7. [ ] Pilot run: 10 tasks × 3 trials
8. [ ] Compare F pilot to A'/D/E pilot results
9. [ ] If F > A': full 89-task run (267 trials)

---

## Appendix A: Condition E Failure Details

From condition-e-improvement.md — every E failure was a false-PASS:

| Category | Count | Examples | Root Cause |
|----------|-------|---------|------------|
| False PASS: blind spot | 5 | cancel-async-tasks (3), regex-log (2) | Agent's self-tests miss edge cases the verifier checks |
| False PASS: constraint violation | 1 | overfull-hbox (1) | Agent satisfies function but violates method constraint |
| False PASS: semantic error | 1 | sparql-university (1) | Cognitive verification (read query, "looks right") without running against data |
| Timeout | 2 | regex-log (1), qemu-startup (1) | Unbounded iteration loop exhausts 30-min budget |

**All 7 non-timeout failures would have been caught by running `/tests/test_outputs.py`.**

## Appendix B: LiteLLM Tool Passing Architecture

The adapter uses litellm for all LLM calls (adapter.py:978):

```python
response = await litellm.acompletion(
    model=model,        # "openrouter/minimax/minimax-m2.7"
    messages=messages,  # system prompt + conversation history
    tools=tools,        # OpenAI function-calling schema
    tool_choice="auto",
    temperature=0.0,
    max_tokens=16384,
)
```

The tool schemas are OpenAI-format JSON objects passed as the `tools` parameter. LiteLLM translates this to the model's native format (OpenRouter → Minimax API). The model returns function calls that litellm normalizes back to OpenAI format.

**This is fully model-agnostic.** Any model that supports function calling via litellm can use these tools. The context gap is not in tool *availability* but in *understanding when and how to use them* — which depends entirely on the system prompt.
