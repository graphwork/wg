# Condition E Design: Autopoietic Organization Generation

**Date:** 2026-04-04  
**Task:** tb-design-condition-e  
**Status:** Design complete — ready for implementation

---

## 1. Context and Motivation

### What we know from A–D

| Condition | Pass rate | What it tests | Key finding |
|-----------|-----------|---------------|-------------|
| A (bare agent) | 48% | Baseline: bash + file tools only | 25% hit 50-turn cap; ~60% mention verification but it's ad-hoc |
| B (wg tools) | 38% | Tools without guidance | 80% ignore wg tools; overhead hurts |
| C (skill injection) | 41% | Tools + explicit teaching | 81% wg adoption; planning phase; still linear execution |
| D (autopoietic verification) | *projected* | Single agent + verify→iterate→converge loop | Removes turn cap; verification as gate; self-termination protocol |

**D's limitation:** A single agent doing everything — implementing AND verifying its own work. Self-assessment is unreliable. The early behavior data shows agents "verify" by re-reading their own code or running it once. A passing `cargo test` doesn't prove correctness if the agent wrote both the code and the test.

**E's thesis:** The agent doesn't solve the problem — it **generates an organization** to solve the problem. Decomposition into independent tasks with **independent verification** (the verifier didn't write the code) and **triage on failure** (spawn new attempts, not self-retry).

### The autopoietic claim

If D is self-verification (a single cell checking itself), E is morphogenesis — the agent creates specialized organs:

- **Implementation tasks** — write code, solve subproblems
- **Verification tasks** — independently check the implementation (the verifier has NO access to the implementer's reasoning, only the artifacts)
- **Triage/retry** — on verification failure, spawn a fresh implementation task with the failure context
- **Convergence** — the whole structure is a cycle that terminates only when the verifier confirms success

This is the core autopoietic pattern: the system produces the components (task graph + agent assignments) that produce the system (verified solution). It's Maturana & Varela's self-producing network applied to a single benchmark trial.

---

## 2. Key Distinction from D

| Dimension | Condition D | Condition E |
|-----------|-------------|-------------|
| **Agent count** | 1 (single agent does everything) | 1 orchestrator → N worker tasks |
| **Verification** | Self-verification (same agent tests its own code) | Independent verification (verifier task didn't write the code) |
| **Failure recovery** | Self-iteration (same agent retries) | Triage: fresh task with failure context (different execution) |
| **Organization** | None — linear attempt→verify→fix loop | Agent GENERATES a task graph (decomposition + verification + triage) |
| **Termination** | wg_done after self-test passes; wg_fail after 3 stuck iterations | Cycle convergence: verifier signals `wg_done --converged` |
| **Decomposition** | Optional ("if 3+ independent phases") | Core mechanism — decomposition IS the strategy |
| **Tool usage** | Same 15 wg tools | Same 15 wg tools + cycle-aware `wg_done --converged` |
| **What it tests** | Does structured self-verification improve pass rate? | Does organization generation improve pass rate? |

E is NOT "D but more." D tests whether a single agent benefits from verification discipline. E tests whether an agent can be a **coordinator** rather than a worker — creating a mini-organization to solve a problem it couldn't solve alone.

---

## 3. Architecture: What E Looks Like in Practice

### 3.1 The Agent's Role

The Condition E agent receives a TB task instruction and acts as an **orchestrator**. It never writes code directly. Instead, it:

1. **Analyzes** the task to determine decomposition strategy
2. **Creates implementation tasks** via `wg_add` with clear descriptions
3. **Creates verification tasks** that depend on implementation tasks
4. **Creates a convergence cycle** — implementation→verification→triage→loop
5. **Monitors** progress via `wg_list` and `wg_show`
6. **Triages** failures by creating new implementation tasks with failure context

### 3.2 Task Graph Template

For a typical TB task, the agent generates a graph like:

```
root-task (the TB task itself)
  ├── impl-1: "Implement solution"
  │     └── verify-1: "Verify implementation" (--after impl-1)
  │           ├── [PASS] → root-task done (--converged)
  │           └── [FAIL] → impl-2: "Fix: <failure diagnosis>" (--after verify-1)
  │                 └── verify-2: "Re-verify" (--after impl-2)
  │                       ├── [PASS] → root-task done (--converged)
  │                       └── [FAIL] → ... (up to max iterations)
  └── [orchestration via wg_list polling]
```

### 3.3 The Cycle Mechanism

**Key constraint:** Harbor runs ONE agent per trial. There is no `wg service` running inside the Docker container. The agent must execute all tasks sequentially — it creates the task graph as a planning/tracking structure, then executes each task itself in order.

This means the "organization" is **cognitive**, not parallel. The agent shifts roles:
- When working on `impl-1`, it's a programmer
- When working on `verify-1`, it's a reviewer (crucially, working from the ARTIFACTS, not from memory of writing the code)
- When triaging a failure, it's a coordinator deciding next steps

The cycle is implemented via **iteration tracking in the prompt**, not via WG's native cycle edges. The agent tracks:
- Current iteration number
- Previous failure context (what the verifier found wrong)
- Accumulated fix history

**Why not native WG cycles?** Native cycles (`--max-iterations`) require the coordinator service to reset tasks. Harbor runs a single agent — there's no coordinator. The cycle must be agent-driven: the agent itself decides when to iterate and when to declare convergence.

### 3.4 Verification Protocol

The verifier task has strict rules:

1. **No access to implementation reasoning.** The verifier reads ONLY the files produced by the implementation (not the impl task's wg_log or reasoning).
2. **Must run the task's test suite** (or equivalent verification command).
3. **Must independently check outputs** — read the produced files and verify they meet the task specification.
4. **Produces a structured verdict:**
   - `PASS` — all tests pass AND manual review finds no issues
   - `FAIL(reason)` — specific, actionable failure description

This is the critical difference from D. In D, the same context window that wrote the code also evaluates it — confirmation bias is inherent. In E, the verification step starts from a clean analytical perspective.

---

## 4. Prompt Specification

### 4.1 Full System Prompt

```python
def build_condition_e_prompt(instruction: str, root_task_id: str, agent_identity: dict) -> str:
    """Condition E: autopoietic organization generation."""
    return (
        "# Task Assignment: Organization Generation Mode\n\n"
        "You are an AI agent completing a Terminal Bench task.\n"
        f"Your root task ID is: **{root_task_id}**\n\n"
        "## Your Identity\n\n"
        f"You are **{agent_identity['name']}** (role: {agent_identity['role']}, "
        f"approach: {agent_identity['tradeoff']}). "
        "You are an ORCHESTRATOR, not a direct implementer. "
        "Your job is to create and manage an organization of tasks "
        "that solves the problem.\n\n"
        "## Core Protocol: Organize → Implement → Verify → Triage\n\n"
        "You MUST follow this protocol:\n\n"
        "### Phase 1: Analyze & Decompose\n"
        "1. Read the task instruction carefully.\n"
        "2. Identify what success looks like (test criteria, expected outputs).\n"
        "3. Break the task into implementation steps.\n"
        "4. Create tasks for each step using `wg_add`.\n\n"
        "### Phase 2: Implement\n"
        "For each implementation task you created:\n"
        "1. Log that you're starting: "
        f'`wg_log("{root_task_id}", "Implementing: <task-name>")`\n'
        "2. Do the implementation work (write code, run commands, etc.)\n"
        "3. Log the result: "
        f'`wg_log("{root_task_id}", "Completed: <task-name>")`\n'
        "4. Mark the subtask done: `wg_done(\"<subtask-id>\")`\n\n"
        "### Phase 3: Independent Verification\n"
        "After ALL implementation tasks are done:\n"
        "1. **STOP and shift perspective.** You are now a REVIEWER, not the implementer.\n"
        "2. **Do NOT rely on your memory of writing the code.** "
        "Instead, read the files fresh as if seeing them for the first time.\n"
        "3. Run the task's test suite or verification command.\n"
        "4. Independently check that outputs match the task specification.\n"
        "5. Record a structured verdict:\n"
        f'   - PASS: `wg_log("{root_task_id}", "VERIFY: PASS — <evidence>")`\n'
        f'   - FAIL: `wg_log("{root_task_id}", "VERIFY: FAIL — <specific issue>")`\n\n'
        "### Phase 4: Triage (on FAIL only)\n"
        "If verification fails:\n"
        "1. Diagnose the root cause from the verification evidence.\n"
        "2. Create a new fix task: "
        '`wg_add("Fix: <diagnosis>", description="Previous attempt failed because: <reason>. Fix: <specific fix>")`\n'
        "3. Implement the fix (Phase 2 again).\n"
        "4. Re-verify (Phase 3 again).\n"
        "5. Repeat until verification passes OR you've done "
        f"{6} iterations without progress.\n\n"
        "### Phase 5: Declare\n"
        f'- Verification passed: `wg_done("{root_task_id}")`\n'
        f'- Stuck after {6} iterations: '
        f'`wg_fail("{root_task_id}", "reason: <what failed across N iterations>")`\n\n'
        "## CRITICAL Rules\n\n"
        f"1. **NEVER call `wg_done(\"{root_task_id}\")` without a PASS verdict.** "
        "The root task represents the TB benchmark task — it can only be done "
        "when verification confirms success.\n"
        "2. **Verification must be INDEPENDENT.** When verifying, read files from disk. "
        "Do not trust your memory of what you wrote. Run tests. Check outputs.\n"
        "3. **Triage creates NEW tasks.** Don't just edit the same code in place — "
        "create a `wg_add(\"Fix: ...\")` task so the fix is tracked. Then implement it.\n"
        "4. **Log everything.** Every phase transition, every verification result, "
        "every triage decision. Your log is the organization's memory.\n"
        "5. **Iterate, don't spin.** Each fix attempt must be DIFFERENT from the last. "
        "If you're trying the same thing twice, step back and reconsider.\n\n"
        "## wg Tools\n\n"
        f'- `wg_log("{root_task_id}", "message")` — Record progress (every phase)\n'
        f'- `wg_done("{root_task_id}")` — Root task complete (ONLY after PASS verdict)\n'
        f'- `wg_fail("{root_task_id}", "reason")` — Cannot complete (with full diagnostics)\n'
        '- `wg_add("title", description="details")` — Create subtasks\n'
        '- `wg_done("<subtask-id>")` — Mark a subtask complete\n'
        f'- `wg_artifact("{root_task_id}", "/path")` — Record output files\n'
        '- `wg_list()` — See all tasks and their status\n'
        '- `wg_show("<task-id>")` — Inspect a task\'s details\n\n'
        "## File Tools\n"
        "- `bash` — Run commands (compile, test, install packages)\n"
        "- `read_file`, `write_file`, `edit_file` — File operations\n"
        "- `glob`, `grep` — Search the codebase\n\n"
        "Begin by reading the task, analyzing what needs to be done, "
        "then creating your implementation plan as wg tasks.\n"
    )
```

### 4.2 Key Differences from D's Prompt

| Aspect | Condition D | Condition E |
|--------|-------------|-------------|
| Agent framing | "You are an agent completing a task" | "You are an ORCHESTRATOR, not a direct implementer" |
| Verification model | "Run tests, check output independently" (but same agent, same context) | "STOP and shift perspective. Read files fresh. Do NOT rely on memory." |
| Decomposition | Optional ("if 3+ independent phases") | Mandatory — "Break the task into implementation steps" |
| Failure recovery | "Diagnose, fix, go back to verify" (in-place fix) | "Create a new fix task via wg_add" (tracked, distinct attempt) |
| Task tracking | wg_log for progress only | wg_add for decomposition + wg_log for progress + wg_done per subtask |
| Iteration limit | 3 stuck iterations | 6 iterations (more attempts, but each is tracked and distinct) |
| Convergence signal | wg_done (simple) | wg_done after explicit PASS verdict |
| Cognitive framing | "Verify your work" | "You are now a REVIEWER, not the implementer" — explicit role shift |

### 4.3 Why This Prompt Should Work Better Than D

The early behavior findings reveal two problems:

1. **Self-assessment bias** (Section 3): ~60% of agents "verify" by running code, but they trust their own implementation. They don't re-read files from a fresh perspective — they check if the thing they just wrote does what they intended, not whether it matches the specification.

2. **Linear execution** (Section 3): Only 35-43% iterate on failure. Most agents fix the first thing they see and move on, without systematic triage.

E addresses both:
- **Role-shifting** ("you are now a REVIEWER") forces a cognitive break between implementation and verification. The prompt explicitly says "do NOT rely on your memory of writing the code."
- **Tracked triage** (wg_add for fix tasks) forces the agent to articulate what went wrong and what it's trying differently, rather than silently editing code.
- **Higher iteration budget** (6 vs D's 3) allows more attempts, but each is distinct and logged.

---

## 5. Adapter Specification: `ConditionEAgent`

### 5.1 Class Definition

```python
class ConditionEAgent(WorkgraphAgent):
    """Condition E: autopoietic organization generation — decompose, verify independently, triage."""

    @staticmethod
    def name() -> str:
        return "wg-condition-e"

    def __init__(self, *args, **kwargs):
        kwargs["condition"] = "E"
        kwargs.setdefault("max_turns", 300)  # higher than D: decomposition + verification + triage
        super().__init__(*args, **kwargs)
```

### 5.2 Setup Changes

Same as D — agency bootstrap + agent identity:

```python
if self.condition in ("B", "C", "D", "E"):
    # existing wg init code...
    
if self.condition in ("D", "E"):
    # Bootstrap agency
    await _exec_wg_cmd_host(wg_dir, wg_bin, ["agency", "init"])
    # Create agent identity
    await _exec_wg_cmd_host(wg_dir, wg_bin, [
        "agent", "create", "orchestrator",
        "--role", "architect",     # E is an architect, not just a programmer
        "--tradeoff", "thorough",  # E prioritizes completeness over speed
    ])
    self._agent_identity = {
        "name": "orchestrator",
        "role": "architect",
        "tradeoff": "thorough",
    }
```

**Note:** E uses "architect" role and "thorough" tradeoff (vs D's "programmer" + "careful") because E's agent is an orchestrator that designs task graphs, not a direct implementer.

### 5.3 Run Changes

```python
elif self.condition == "E":
    tools = CONDITION_E_TOOLS  # same 15 tools
    root_task_id = f"tb-{uuid.uuid4().hex[:8]}"
    title = instruction[:100] + ("..." if len(instruction) > 100 else "")
    await _exec_wg_cmd_host(wg_dir, wg_bin, ["add", title, "--id", root_task_id])
    await _exec_wg_cmd_host(wg_dir, wg_bin, ["assign", root_task_id, "orchestrator"])
    system_prompt = build_condition_e_prompt(
        instruction, root_task_id, self._agent_identity
    )
```

### 5.4 Enhanced Tracking

Same verification/termination tracking as D, plus organization-specific metrics:

```python
# Additional tracking for E:
decomposition_tasks = []     # list of subtask IDs created via wg_add
verification_verdicts = []   # list of (iteration, "PASS"/"FAIL", reason)
triage_count = 0             # number of fix tasks created after verification failure
max_decomposition_depth = 0  # deepest subtask nesting level

# In the tool call loop:
if fn_name == "wg_add":
    decomposition_tasks.append(fn_args.get("title", ""))
    
# Parse wg_log calls for VERIFY verdicts:
if fn_name == "wg_log" and "VERIFY:" in fn_args.get("message", ""):
    msg = fn_args["message"]
    if "PASS" in msg:
        verification_verdicts.append((turn, "PASS", msg))
    elif "FAIL" in msg:
        verification_verdicts.append((turn, "FAIL", msg))
        
if fn_name == "wg_add" and fn_args.get("title", "").startswith("Fix:"):
    triage_count += 1
```

### 5.5 Metadata

```python
context.metadata = {
    "condition": "E",
    "turns": turn_count,
    "root_task_id": root_task_id,
    "model": model,
    "agent_identity": self._agent_identity,
    # D-compatible metrics:
    "verification_iterations": verification_count,
    "self_termination_type": termination_type,
    "wg_tool_calls": wg_tool_call_count,
    "verification_commands": verification_commands_list,
    # E-specific metrics:
    "decomposition_task_count": len(decomposition_tasks),
    "decomposition_tasks": decomposition_tasks[:20],  # cap for metadata size
    "verification_verdicts": verification_verdicts,
    "triage_count": triage_count,
    "organization_phases": {
        "decompose": bool(decomposition_tasks),
        "verify_independent": any(v[1] for v in verification_verdicts),
        "triage_on_fail": triage_count > 0,
    },
}
```

### 5.6 Tool Set

```python
CONDITION_E_TOOLS = CONDITION_B_TOOLS  # identical 15 tools
```

The differentiator is the prompt and tracking, not the tools. Same principle as C vs B.

---

## 6. Harbor Integration: How It Runs Inside a Trial

### 6.1 Constraints

Harbor runs **one agent per trial** inside a Docker container. There is:
- No `wg service` running (no coordinator, no agent spawning)
- No parallel execution (one LLM conversation loop)
- No inter-agent communication (one agent, multiple cognitive roles)

### 6.2 How the Cycle Works

The "cycle" in E is **agent-driven, not system-driven.** The agent maintains a loop counter in its own context:

```
Iteration 1:
  → Implement solution
  → Verify (role shift)
  → FAIL → create Fix task with diagnosis

Iteration 2:
  → Implement fix (from triage)
  → Verify again (role shift)
  → PASS → wg_done root task

(or FAIL → Iteration 3, up to max 6)
```

The wg task graph records this structure — each iteration's implementation and verification are distinct tracked tasks. But the execution is sequential within a single agent loop.

### 6.3 Why Not Native WG Cycles?

Native WG cycles (`--max-iterations`) require:
1. A coordinator service running to detect cycle completion and reset tasks
2. Multiple agent processes that can be independently spawned

Neither is available in a Harbor trial. The agent must manage the cycle itself.

However, the **wg task graph still records the organizational structure.** Post-trial analysis can reconstruct the cycle iterations from the task graph and logs. This is valuable for measuring decomposition behavior, verification quality, and triage effectiveness.

### 6.4 Harbor Run Command

```bash
harbor run \
    -d terminal-bench/terminal-bench-2 \
    --agent-import-path "wg.adapter:ConditionEAgent" \
    -m "openrouter/minimax/minimax-m2.7" \
    -k 3 \
    -n 4 \
    --job-name condition-e \
    --jobs-dir terminal-bench/results/condition-e \
    --no-delete \
    --debug \
    --ak "max_turns=300" \
    --ak "temperature=0.0" \
    --agent-timeout-multiplier 8.0 \
    -y \
    2>&1 | tee terminal-bench/results/condition-e/run.log
```

### 6.5 Parameter Rationale

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| `max_turns=300` | Safety valve | E uses more turns than D: decomposition planning + N implementation phases + N verification phases + triage. 300 is ~6x D's expected mean (~50 turns). |
| `--agent-timeout-multiplier 8.0` | ~40 min per trial | Higher than D's 6x because E's multi-phase execution takes longer. Still bounded. |
| `-k 3` | 3 trials per task | Same as A/D for statistical comparability |
| `-n 4` | 4 concurrent | Same as A/D |
| Same model | minimax-m2.7 | Controlled variable |

---

## 7. Cost and Time Estimates

### 7.1 Token Cost Model

E uses more tokens per trial than D because of:
1. **Decomposition phase** — additional tool calls to create subtasks (~5-10 extra turns)
2. **Role-shift verification** — re-reading files that were just written (~1.5x D's verification cost)
3. **Triage iterations** — when verification fails, creating fix tasks + re-implementing (~2x per retry iteration)
4. **Organizational overhead** — wg_add, wg_list, wg_show calls add ~3-5 turns per iteration

Based on Condition A data (203 trials, mean ~27 turns, ~$0.60/trial):

| Scenario | Mean turns/trial | Multiplier vs A | Cost/trial | 267 trials |
|----------|-----------------|-----------------|------------|------------|
| Optimistic (pass on 1st verify) | 45 | 1.7x | $1.02 | $272 |
| Expected (1 retry average) | 65 | 2.4x | $1.44 | $384 |
| Pessimistic (3 retries average) | 100 | 3.8x | $2.28 | $609 |

### 7.2 Turn Budget Breakdown (Expected Case)

| Phase | Turns | Notes |
|-------|-------|-------|
| Analysis + decomposition | 5-8 | Read task, create wg_add tasks, plan |
| Implementation (attempt 1) | 20-30 | Similar to A's typical solve time |
| Verification (attempt 1) | 5-10 | Re-read files, run tests, render verdict |
| Triage + fix (1 iteration) | 15-20 | Diagnosis + targeted fix |
| Re-verification | 5-10 | Run tests again |
| Total (1 retry) | **50-78** | Mean ~65 turns |

### 7.3 Comparison Table

| Condition | Cost/trial | 267 trials | Wall-clock (projected) |
|-----------|-----------|------------|----------------------|
| A | $0.60 | $160 | ~5 hours |
| D (expected) | $0.81 | $216 | ~9 hours |
| **E (expected)** | **$1.44** | **$384** | **~14 hours** |
| E (pessimistic) | $2.28 | $609 | ~22 hours |

### 7.4 Cost-Effectiveness Threshold

For E to be cost-effective vs A:
- A: 48% pass at $0.60/trial = $1.25 per success
- E must achieve: >$1.44 × 48% / $0.60 = **>115% of A's pass rate** to match cost-effectiveness
- In absolute terms: E needs **>58% pass rate** to be cheaper per success than A at $1.44/trial

For E to be cost-effective vs D (projected):
- E needs pass_rate_E / $1.44 > pass_rate_D / $0.81
- If D achieves 55% (optimistic): E needs >97% at $1.44 (unlikely)
- If D achieves 50%: E needs >89% (unlikely for first run)

**Realistic expectation:** E will likely cost 2-3x more per trial than A. The value is in the **mechanism** — if independent verification catches bugs that self-verification misses, the pass rate on HARD tasks (where A < 30%) could see a disproportionate lift, even if overall cost-effectiveness is lower.

---

## 8. Metrics Collection

### 8.1 Primary Metrics (Hypothesis Testing)

| Metric | What it tests | E vs D hypothesis |
|--------|--------------|-------------------|
| **Pass rate** | Overall success | E > D (independent verification catches more bugs) |
| **Pass rate on hard tasks** | Success on A < 30% tasks | Where the biggest lift should appear |
| **Verification accuracy** | Does PASS verdict correlate with actual pass? | E's independent verification should be more reliable than D's self-check |
| **Retry success rate** | % of trials that fail verification, retry, and then pass | E's tracked triage should improve retry quality |

### 8.2 Organization-Specific Metrics

| Metric | Source | What it reveals |
|--------|--------|----------------|
| `decomposition_task_count` | metadata | How many subtasks the agent creates |
| `verification_verdicts` | metadata | PASS/FAIL sequence — how many iterations to convergence |
| `triage_count` | metadata | How many fix attempts were needed |
| `organization_phases` | metadata | Did the agent actually decompose? Verify independently? Triage? |
| Subtask completion rate | workgraph_state | What % of created subtasks were completed |
| Verification FAIL→fix→PASS rate | verdicts | How effective is the triage mechanism |
| False PASS rate | verdicts vs benchmark result | Agent says PASS but benchmark says FAIL — verification quality |
| False FAIL rate | verdicts vs benchmark result | Agent says FAIL but the solution was actually correct |

### 8.3 Analysis Plan

1. **E vs A/D pass rate** — primary outcome. Fisher's exact test per task.
2. **Verification accuracy** — confusion matrix: (agent verdict) × (benchmark result). Measures whether "independent" verification is actually better than D's self-check.
3. **Organization compliance** — what % of trials actually follow the decompose→verify→triage protocol? (Same kind of compliance analysis as B/C wg adoption rates.)
4. **Triage effectiveness** — among trials where verification fails: what % recover via triage vs what % exhaust iterations?
5. **Cost-effectiveness** — pass rate per dollar across conditions.
6. **Hard task analysis** — on the 20 hardest tasks (A < 30%), does E's organization strategy provide disproportionate lift?
7. **Decomposition depth vs success** — does creating more subtasks correlate with success, or does over-decomposition hurt?

---

## 9. Org Generation Prompt/Skill Spec

The skill that teaches the agent to BUILD an organization is embedded in the system prompt (Section 4.1). The key elements:

### 9.1 What Makes This a "Skill"

The prompt teaches a **cognitive strategy**, not just tool usage:

1. **Role-shifting** — "STOP and shift perspective. You are now a REVIEWER." This is the core cognitive technique. The agent must mentally reset between implementation and verification.

2. **Tracked decomposition** — Every implementation step is a `wg_add` task. This forces explicit planning and creates an audit trail.

3. **Structured verdicts** — "VERIFY: PASS" / "VERIFY: FAIL" format in wg_log. This creates machine-parseable verification records.

4. **Escalating triage** — Fix tasks are NEW tasks (`wg_add("Fix: ...")`), not in-place edits. This forces the agent to articulate the diagnosis and planned fix before acting.

### 9.2 Skill Injection Points

The skill is injected via the system prompt (same mechanism as C and D). No additional injection points needed — the system prompt is the single control surface in Harbor's architecture.

---

## 10. Verification Task Template

When the E agent reaches Phase 3 (verification), the protocol it follows:

```
VERIFICATION PROTOCOL
═════════════════════

Context: You just completed implementation. Now verify independently.

1. STOP. Clear your mental model of what you just implemented.

2. Read the original task specification (the instruction).
   What EXACTLY is required?

3. Read the produced files FROM DISK (read_file, not memory).
   What EXACTLY was produced?

4. Run the test suite:
   bash("run-tests.sh") or equivalent

5. Check outputs against specification:
   - Does the output format match?
   - Do edge cases work?
   - Are there off-by-one errors?
   - Does it handle the examples in the spec?

6. Render verdict:
   PASS — "All N tests pass. Output matches spec for cases X, Y, Z."
   FAIL — "Test N fails: expected X, got Y. Root cause: Z."
```

This template is embedded in the prompt (Section 4.1, Phase 3). The agent doesn't receive it as a separate file — it's part of the cognitive strategy.

---

## 11. Cycle Configuration Spec

### 11.1 Agent-Driven Cycle (Harbor Context)

Since Harbor runs a single agent, the cycle is managed in the prompt:

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| Max iterations | 6 | Higher than D's 3 because each iteration is tracked (not wasted). 6 attempts gives meaningful retry data while preventing infinite loops. |
| Convergence signal | `wg_done(root_task_id)` after PASS verdict | Same as D — the root task is the convergence point. |
| Failure signal | `wg_fail(root_task_id, reason)` after 6 FAIL iterations | Exhaustion with full diagnostics. |
| Iteration tracking | wg_log with `VERIFY: PASS/FAIL` | Machine-parseable verdict history. |
| Fix tracking | `wg_add("Fix: ...")` | Each fix attempt is a distinct task with description. |

### 11.2 Future: Native WG Cycles (Post-Harbor)

If/when TB trials support multi-agent execution (wg service running), E's cycle could be expressed natively:

```bash
# Implementation task (cycle member)
wg add "Implement solution" --id impl --after root

# Verification task (cycle member)
wg add "Verify solution" --id verify --after impl

# Back-edge: verify → impl (cycle)
wg add "Implement solution" --id impl --after verify  # creates cycle edge
wg config --cycle impl --max-iterations 6 --delay 0

# Convergence: verifier signals wg done --converged
```

This is documented for future reference but NOT part of the current Harbor implementation.

---

## 12. Specific Adapter Code Changes

All changes are in `terminal-bench/wg/adapter.py`.

### 12.1 New Constants

```python
CONDITION_E_TOOLS = CONDITION_B_TOOLS  # Same 15 tools
```

### 12.2 New Prompt Builder

Add `build_condition_e_prompt()` as specified in Section 4.1.

### 12.3 Modified `WorkgraphAgent.setup()`

Change:
```python
if self.condition in ("B", "C"):
```
To:
```python
if self.condition in ("B", "C", "D", "E"):
```

Add E-specific setup (shared with D, using "orchestrator" identity):
```python
if self.condition == "E":
    await _exec_wg_cmd_host(wg_dir, wg_bin, ["agency", "init"])
    await _exec_wg_cmd_host(wg_dir, wg_bin, [
        "agent", "create", "orchestrator",
        "--role", "architect",
        "--tradeoff", "thorough",
    ])
    self._agent_identity = {
        "name": "orchestrator",
        "role": "architect",
        "tradeoff": "thorough",
    }
```

### 12.4 Modified `WorkgraphAgent.run()`

Add E branch:
```python
elif self.condition == "E":
    tools = CONDITION_E_TOOLS
    root_task_id = f"tb-{uuid.uuid4().hex[:8]}"
    title = instruction[:100] + ("..." if len(instruction) > 100 else "")
    await _exec_wg_cmd_host(wg_dir, wg_bin, ["add", title, "--id", root_task_id])
    await _exec_wg_cmd_host(wg_dir, wg_bin, ["assign", root_task_id, "orchestrator"])
    system_prompt = build_condition_e_prompt(
        instruction, root_task_id, self._agent_identity
    )
```

Add E-specific tracking (decomposition, verdicts, triage) per Section 5.4.

### 12.5 New Class

```python
class ConditionEAgent(WorkgraphAgent):
    """Condition E: autopoietic organization generation."""

    @staticmethod
    def name() -> str:
        return "wg-condition-e"

    def __init__(self, *args, **kwargs):
        kwargs["condition"] = "E"
        kwargs.setdefault("max_turns", 300)
        super().__init__(*args, **kwargs)
```

### 12.6 Save wg state (add "E" to condition check)

Change:
```python
if self.condition in ("B", "C") and wg_dir:
```
To:
```python
if self.condition in ("B", "C", "D", "E") and wg_dir:
```

### 12.7 Module Docstring Update

```python
"""
Terminal Bench Agent Adapter for Harbor Framework.

Supports five conditions:
  Condition A (control): bash + file tools only, no graph, no resume
  Condition B (treatment): full wg tool access, graph awareness, journal/resume
  Condition C (treatment): wg tools + skill injection + planning phase
  Condition D (treatment): wg tools + autopoietic verification + agency identity
  Condition E (treatment): wg tools + organization generation + independent verification
"""
```

---

## 13. Risk Analysis

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| **Agent skips decomposition** — solves directly like D | High | E degrades to D (no org generation) | "You are an ORCHESTRATOR, not a direct implementer" + "Break the task into implementation steps" (mandatory, not conditional). Monitor organization_phases.decompose in metadata. |
| **Agent can't role-shift** — verification is still self-assessment | Medium | Independent verification promise is hollow | "STOP and shift perspective" + "read files FROM DISK" + "do NOT rely on your memory." Measure false PASS rate vs D to test this. |
| **Decomposition overhead kills simple tasks** | High | Simple tasks (A > 80%) take 2x longer with no benefit | Acceptable cost — E targets HARD tasks. Simple task overhead is the price of a general strategy. Monitor per-task-difficulty pass rates. |
| **300-turn safety valve hit** | Low | Agent spins in triage loop | 6-iteration hard cap in prompt. 300 turns is ~5x expected mean. Monitor. |
| **Cost blowup** | Medium | $600+ for full run | $609 pessimistic upper bound is acceptable. Monitor early — if mean cost/trial exceeds $3, pause. |
| **wg_add task IDs unpredictable** | Low | Agent can't reference subtasks by ID | wg_add returns the auto-generated task ID in its output. Agent can parse this. |
| **Organizational structure varies wildly** | Medium | Hard to compare across trials | This IS the experiment — we WANT to see how the agent chooses to organize. Track decomposition depth and structure in metadata. |

---

## 14. Pre-Flight Checklist

```bash
# 1. Build static wg binary
cd /home/erik/workgraph
cargo build --release --target x86_64-unknown-linux-gnu

# 2. Pre-pull Docker images
bash terminal-bench/pre-pull-images.sh

# 3. Verify adapter loads
python3 -c "from wg.adapter import ConditionEAgent; print(ConditionEAgent.name())"

# 4. Smoke test (1 task, 1 trial)
harbor run \
    -d terminal-bench/terminal-bench-2 \
    --agent-import-path "wg.adapter:ConditionEAgent" \
    -m "openrouter/minimax/minimax-m2.7" \
    -k 1 -n 1 \
    --job-name condition-e-smoke \
    --jobs-dir terminal-bench/results/condition-e-smoke \
    --no-delete --debug \
    --ak "max_turns=300" --ak "temperature=0.0" \
    --agent-timeout-multiplier 8.0 \
    -i "constraints-scheduling" \
    -y
# Verify: trial completes, agent creates subtasks, verification verdict logged,
# workgraph_state has multiple tasks + agency data
```

---

## 15. Implementation Checklist

1. [ ] Add `build_condition_e_prompt()` to adapter.py
2. [ ] Add `CONDITION_E_TOOLS = CONDITION_B_TOOLS`
3. [ ] Add `ConditionEAgent` class
4. [ ] Modify `setup()` to handle condition "E" (agency init + orchestrator agent)
5. [ ] Modify `run()` to route condition "E" (prompt + assignment + tracking)
6. [ ] Add E-specific tracking (decomposition, verdicts, triage) to tool loop
7. [ ] Add enhanced metadata fields to `context.metadata`
8. [ ] Add "E" to wg state save condition
9. [ ] Update module docstring
10. [ ] Smoke test with 1 trial on `constraints-scheduling`
11. [ ] Verify workgraph_state has decomposed task graph
12. [ ] Verify verification verdicts appear in metadata
13. [ ] Full run: 267 trials (89 tasks x 3)
14. [ ] Analysis: E vs A, E vs D comparison

---

## Appendix A: Experimental Conditions Summary

| | A (baseline) | B (cursed) | C (cursed) | D (self-verify) | **E (org generation)** |
|---|---|---|---|---|---|
| Tools | 6 (bash+file) | 15 (+wg) | 15 (+wg) | 15 (+wg) | **15 (+wg)** |
| Prompt style | Minimal | Tools listed | Skill injection + planning | Autopoietic verify loop | **Organization generation protocol** |
| wg init | No | Yes | Yes | Yes + agency | **Yes + agency** |
| Agency identity | No | No | No | solver/programmer/careful | **orchestrator/architect/thorough** |
| Decomposition | N/A | Optional | Conditional | Optional | **Mandatory** |
| Verification | None | None | None | Self-check (same agent) | **Independent (role-shift + re-read)** |
| Failure recovery | None | wg_fail (rare) | wg_fail | Self-fix (3 iterations) | **Tracked triage (6 iterations via wg_add)** |
| Turn limit | 50 | 50 | 50 | 200 (safety valve) | **300 (safety valve)** |
| Timeout | Default | Default | Default | 6x default | **8x default** |
| Self-termination | Stop generating | wg_done (44%) | wg_done (86%) | wg_done/fail (target 95%+) | **wg_done/fail (target 95%+)** |
| Cost/trial | $0.60 | ~$0.60 | ~$0.60 | ~$0.81 | **~$1.44** |

## Appendix B: Why "Organization" Not "Multi-Agent"

E does NOT create multiple agents in the Harbor sense. It creates multiple TASKS that a single agent executes sequentially. The "organization" is:

1. **Cognitive** — the agent shifts between implementer and reviewer roles
2. **Structural** — the task graph records the decomposition, creating an audit trail
3. **Strategic** — triage decisions are tracked as new tasks, not invisible edits

This is different from true multi-agent systems (where separate LLM instances independently solve subtasks). True multi-agent would require:
- `wg service start` inside the Docker container
- Multiple concurrent LLM conversations
- Harbor support for long-running containers with internal services

These are possible future extensions but NOT part of Condition E. The value E tests is: **does the cognitive strategy of organization generation (decompose + role-shift verify + tracked triage) improve outcomes, even when executed by a single agent?**

If E succeeds, the natural next step is Condition F: true multi-agent (wg service running inside trial, separate LLM instances for implementation and verification). But that requires Harbor infrastructure changes that are out of scope for now.

## Appendix C: Design Questions Answered

| # | Question | Answer |
|---|----------|--------|
| 1 | How does the agent know HOW to decompose? | System prompt teaches the protocol (Section 4.1): analyze → create wg_add tasks → implement → verify → triage. |
| 2 | How does independent verification work inside Docker? | Same agent, same env, but explicit ROLE SHIFT in prompt. "Read files from disk, not memory." Cognitive independence, not process independence. |
| 3 | How do we handle the cycle inside Harbor? | Agent-driven cycle (Section 6.2): agent tracks iterations in its own context. Not native WG cycles (no service running). |
| 4 | Cost model? | Expected $1.44/trial (2.4x A), $384 for full run. Pessimistic $2.28/trial, $609 total (Section 7). |
| 5 | How do we measure? | Decomposition depth, verification verdicts (PASS/FAIL), triage count, cycle iterations, total tokens, wall-clock time — all in metadata (Section 8). |
| 6 | Max iterations? | 6 (Section 11.1). Higher than D's 3 because each iteration is tracked and distinct. |
| 7 | Agent-decided vs prescribed org structure? | Agent-decided (Section 3.1). The prompt teaches the PROTOCOL, not the structure. The agent decides how many subtasks, what granularity, when to verify. This is the autopoietic thesis — the system generates its own structure. |
