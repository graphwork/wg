# Condition D Design: Properly-Configured wg-Augmented Agent

**Date:** 2026-04-04  
**Task:** tb-design-condition-d  
**Status:** Design complete — ready for implementation

---

## 1. Context and Motivation

Condition A (bare agent, no wg tools) achieved a **48% pass rate** on terminal-bench-2 with minimax-m2.7. This is our valid baseline.

Conditions B and C were **cursed replicates**:
- **B**: Gave wg tools but no guidance → 80% of agents ignored them → 38% pass rate (worse than A)
- **C**: Added skill injection + planning phase → partially recovered to 41%, but still <A
- **B-rerun**: Accidentally ran ConditionCAgent → data invalidated for B vs C comparison
- **Neither B nor C**: initialized `wg` properly (no `wg service`, no agency, no assignments)
- **Both B and C**: used `max_turns=50`, which penalized tool-heavy agents (25% of A trials hit the cap; wg tool calls count against it)

Condition D is the **first fair test** of whether wg augmentation helps. It must fix every issue above.

---

## 2. Design Requirements (from task description)

| # | Requirement | How D addresses it |
|---|---|---|
| 1 | wg properly initialized | `wg init` + agency bootstrap + root task created in setup() |
| 2 | No turn limit | Remove `--ak max_turns=50`. Agent loop runs until natural termination or wall-clock timeout |
| 3 | Generous timeout | `--agent-timeout-multiplier 6.0` (~30 min effective per trial) + `max_turns=200` safety valve |
| 4 | Autopoietic verification loop | Prompt teaches attempt → verify → iterate → declare. `wg_done` requires prior verification step |
| 5 | Correct adapter | New `ConditionDAgent` class, dedicated prompt builder |
| 6 | Agency integration | Agent gets role name + tradeoff context in prompt; `wg assign` called in setup() |
| 7 | Metrics | All existing metrics + verification_iterations, self_termination_type, wall_clock_seconds |

---

## 3. Adapter Specification: `ConditionDAgent`

### 3.1 Class Definition

```python
class ConditionDAgent(WorkgraphAgent):
    """Condition D: wg tools + autopoietic verification + agency identity + no turn cap."""

    @staticmethod
    def name() -> str:
        return "wg-condition-d"

    def __init__(self, *args, **kwargs):
        kwargs["condition"] = "D"
        kwargs.setdefault("max_turns", 200)  # safety valve, not a cap
        super().__init__(*args, **kwargs)
```

### 3.2 Changes to `WorkgraphAgent.__init__`

Add `"D"` to the condition validation and tool/prompt routing.

### 3.3 Tool Set

**Same 15 tools as B/C** (CONDITION_B_TOOLS). The variable is the prompt and setup, not the tools.

```python
CONDITION_D_TOOLS = CONDITION_B_TOOLS  # identical tool set
```

### 3.4 Setup Changes (`setup()` method)

Current B/C setup does `wg init` and creates a root task. D must additionally:

1. **Bootstrap agency** — seed starter roles/tradeoffs so the agent has identity context:
   ```python
   # After wg init:
   await _exec_wg_cmd_host(wg_dir, wg_bin, ["agency", "init"])
   ```

2. **Create agent identity** — create an agent with a role and tradeoff:
   ```python
   # Create a "Solver" agent with "Thorough" tradeoff
   await _exec_wg_cmd_host(wg_dir, wg_bin, [
       "agent", "create", "solver",
       "--role", "programmer",   # from starter data
       "--tradeoff", "careful",  # from starter data
   ])
   ```

3. **Assign agent to root task**:
   ```python
   # After creating root task:
   await _exec_wg_cmd_host(wg_dir, wg_bin, [
       "assign", root_task_id, "solver"
   ])
   ```

4. **Store assignment metadata** for analysis:
   ```python
   self._agent_identity = {
       "name": "solver",
       "role": "programmer",
       "tradeoff": "careful",
   }
   ```

### 3.5 Run Loop Changes

The `run()` method in `WorkgraphAgent` needs these modifications for condition D:

1. **Remove turn cap enforcement** — `max_turns=200` is a safety valve, not a behavioral constraint. The agent should self-terminate via `wg_done` or `wg_fail`, not by running out of turns.

2. **Track verification iterations** — count how many times the agent calls verification-related bash commands (commands containing `test`, `check`, `verify`, `pytest`, `make test`, etc.) followed by a fix attempt:
   ```python
   # In the tool execution loop, track patterns:
   if fn_name == "bash" and is_verification_command(fn_args.get("command", "")):
       verification_count += 1
   ```

3. **Track self-termination type** — record how the agent ended:
   - `"wg_done"` — called wg_done on root task
   - `"wg_fail"` — called wg_fail (gave up with diagnostics)
   - `"no_tool_calls"` — LLM returned no tool calls (natural stop)
   - `"max_turns"` — hit the 200-turn safety valve
   - `"timeout"` — wall-clock timeout from Harbor

4. **Enhanced metadata in `context.metadata`**:
   ```python
   context.metadata = {
       "condition": "D",
       "turns": turn_count,
       "root_task_id": root_task_id,
       "model": model,
       "agent_identity": self._agent_identity,
       "verification_iterations": verification_count,
       "self_termination_type": termination_type,
       "wg_tool_calls": wg_tool_call_count,
       "verification_commands": verification_commands_list,
   }
   ```

---

## 4. Prompt Specification: Autopoietic Verification Loop

This is the core differentiator. The prompt teaches the agent a **self-sustaining verification cycle**: attempt → test → iterate → converge.

### 4.1 Full System Prompt

```python
def build_condition_d_prompt(instruction: str, root_task_id: str, agent_identity: dict) -> str:
    """Condition D: autopoietic verification loop + agency identity + wg tools."""
    return (
        "# Task Assignment\n\n"
        "You are an AI agent completing a Terminal Bench task.\n"
        f"Your root task ID is: **{root_task_id}**\n\n"
        "## Your Identity\n\n"
        f"You are **{agent_identity['name']}** (role: {agent_identity['role']}, "
        f"approach: {agent_identity['tradeoff']}). "
        "This means you prioritize correctness over speed. "
        "Verify your work before declaring it done.\n\n"
        "## Core Loop: Attempt → Verify → Iterate → Declare\n\n"
        "You MUST follow this loop for every task:\n\n"
        "1. **Understand**: Read the task. Identify what success looks like. "
        "Find any existing tests or verification criteria.\n"
        "2. **Attempt**: Implement your solution.\n"
        "3. **Verify**: Run the task's tests, check command, or verify output independently. "
        "Do NOT rely on your own reading of the code — execute something that proves correctness.\n"
        "4. **Iterate**: If verification fails, diagnose the failure, fix it, and go back to step 3. "
        "You may iterate as many times as needed.\n"
        "5. **Declare**:\n"
        f'   - If verification passes: `wg_done("{root_task_id}")`\n'
        f'   - If you are stuck after 3+ failed iterations and cannot make progress: '
        f'`wg_fail("{root_task_id}", "reason: what failed and what you tried")`\n\n'
        "**CRITICAL**: Never call `wg_done` without first running a verification step "
        "that succeeded. Never spin indefinitely — if 3 consecutive fix attempts fail "
        "on the same issue, call `wg_fail` with diagnostics.\n\n"
        "## wg Tools\n\n"
        f'- `wg_log("{root_task_id}", "message")` — Record progress (do this at each step)\n'
        f'- `wg_done("{root_task_id}")` — Task complete (ONLY after verification passes)\n'
        f'- `wg_fail("{root_task_id}", "reason")` — Cannot complete (with diagnostics)\n'
        f'- `wg_add("title")` — Decompose into subtasks if needed\n'
        f'- `wg_artifact("{root_task_id}", "/path")` — Record output files\n\n'
        "Use `wg_log` at every major step. This is your external memory — "
        "if your context fills up, a resumed agent can read your log.\n\n"
        "## When to Decompose\n\n"
        "If the task has 3+ independent phases that could fail independently, "
        "decompose with `wg_add`. Otherwise, solve directly. "
        "Most tasks are single-phase — just use the core loop.\n\n"
        "## Tools Available\n"
        "- `bash` — Run commands (compile, test, install packages)\n"
        "- `read_file`, `write_file`, `edit_file` — File operations\n"
        "- `glob`, `grep` — Search the codebase\n"
        "- `wg_*` tools — Task coordination (see above)\n\n"
        "Begin by reading the task, identifying verification criteria, then implementing.\n"
    )
```

### 4.2 Key Differences from C's Prompt

| Aspect | Condition C | Condition D |
|--------|-------------|-------------|
| Planning phase | Mandatory "analyze in ONE response" before acting | No forced planning — just "understand first" |
| Verification | Not mentioned | Core loop: verify before declaring done, iterate on failure |
| Failure protocol | `wg_fail` with reason | `wg_fail` after 3+ stuck iterations with diagnostics |
| Identity | None | Agent name + role + tradeoff in prompt |
| `wg_done` gate | No gate — call anytime | "NEVER call without successful verification" |
| Decomposition guidance | "If 3+ phases or might exhaust context" | "If 3+ independent phases that could fail independently" |
| Logging rationale | "Enables crash recovery" | "Your external memory — resumed agent reads it" |

### 4.3 Why This Prompt Should Work

The early behavior findings (Section 3) revealed:
- **60% of agents already mention "verify/test/check"** — the behavior exists but is ad-hoc
- **Only 35-43% iterate on failure** — most give up or proceed linearly
- **No formal verification loop** — agents don't have attempt→test→fix→retest discipline
- **45% of failures hit the turn limit** — agents spin without knowing when to stop

Condition D's prompt addresses each:
- Formalizes the verify step as a **gate** (not optional)
- Teaches **iteration** as the expected behavior, not a fallback
- Provides a **convergence criterion** (3 stuck iterations → fail with diagnostics)
- Removes the turn cap so agents aren't punished for iterating

---

## 5. Harbor Run Configuration

### 5.1 Run Command

```bash
harbor run \
    -d terminal-bench/terminal-bench-2 \
    --agent-import-path "wg.adapter:ConditionDAgent" \
    -m "openrouter/minimax/minimax-m2.7" \
    -k 3 \
    -n 4 \
    --job-name condition-d \
    --jobs-dir terminal-bench/results/condition-d \
    --no-delete \
    --debug \
    --ak "max_turns=200" \
    --ak "temperature=0.0" \
    --agent-timeout-multiplier 6.0 \
    -y \
    2>&1 | tee terminal-bench/results/condition-d/run.log
```

### 5.2 Parameter Rationale

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| `-k 3` | 3 trials per task | Same as A for statistical comparability |
| `-n 4` | 4 concurrent | Same as A |
| `-m openrouter/minimax/minimax-m2.7` | Same model | Controlled variable |
| `max_turns=200` | Safety valve | ~4x the typical completion (mean 26 turns in A). Not a cap — agents should self-terminate via wg_done/wg_fail |
| `temperature=0.0` | Deterministic | Same as A |
| `--agent-timeout-multiplier 6.0` | ~30 min per trial | Default timeout × 6. Generous enough for complex tasks with iteration. Based on: A trials complete in ~5 min typical |
| `--no-delete` | Preserve containers | Docker image cache preservation |

### 5.3 Pre-flight Checklist

```bash
# 1. Build static wg binary
cd /home/erik/workgraph
cargo build --release --target x86_64-unknown-linux-gnu
# Verify: target/x86_64-unknown-linux-gnu/release/wg exists

# 2. Pre-pull Docker images
bash terminal-bench/pre-pull-images.sh

# 3. Verify adapter loads
python3 -c "from wg.adapter import ConditionDAgent; print(ConditionDAgent.name())"

# 4. Smoke test (1 task, 1 trial)
harbor run \
    -d terminal-bench/terminal-bench-2 \
    --agent-import-path "wg.adapter:ConditionDAgent" \
    -m "openrouter/minimax/minimax-m2.7" \
    -k 1 -n 1 \
    --job-name condition-d-smoke \
    --jobs-dir terminal-bench/results/condition-d-smoke \
    --no-delete --debug \
    --ak "max_turns=200" --ak "temperature=0.0" \
    --agent-timeout-multiplier 6.0 \
    -i "constraints-scheduling" \
    -y
# Verify: trial completes, agent calls wg_done, workgraph_state/ has agency data
```

---

## 6. Cost and Time Estimates

### 6.1 Token Cost

Based on Condition A results (203 trials with token data):

| Metric | Condition A | Condition D (projected) |
|--------|------------|------------------------|
| Mean input tokens/trial | 694,627 | ~937,000 (1.35x) |
| Mean output tokens/trial | 13,773 | ~18,600 (1.35x) |
| Mean turns/trial | 26.5 | ~35 (uncapped trials go longer) |
| Trials hitting cap | 25% (at 50) | ~5% (at 200 safety valve) |
| Cost per trial | $0.60 | ~$0.81 |
| **267 trials (89×3)** | **$160** | **$216** |

**The 1.35x factor** accounts for the 25% of trials that previously hit the 50-turn cap now running to natural completion (~80-100 turns). This is conservative — if verification loops add more iterations, actual cost could be 1.5-2x ($270-$430).

**Upper bound estimate: $430** (if average trial doubles for the 25% that were capped + 10% overhead from verification iterations).

### 6.2 Wall-Clock Time

| Factor | Estimate |
|--------|----------|
| Trials per concurrency slot | 267 / 4 = 67 batches |
| Time per trial (mean) | ~8 min (A was ~5 min, D has more iterations) |
| **Total wall-clock** | **~9 hours** |

### 6.3 Budget Summary

| Scenario | Cost | Time |
|----------|------|------|
| Optimistic (1.2x A) | $192 | 7 hours |
| Expected (1.35x A) | $216 | 9 hours |
| Conservative (2x A) | $430 | 14 hours |

---

## 7. Metrics Collection

### 7.1 Primary Metrics (Hypothesis Testing)

| Metric | What it measures | Expected comparison |
|--------|------------------|-------------------|
| **Pass rate** | Task success % | D > A if wg augmentation helps |
| **Pass rate on hard tasks** | Success on tasks where A < 50% | Where the lift should be largest |
| **Turns to completion** | Efficiency | D may use more turns (verification iterations) — this is fine |
| **Self-termination rate** | % ending via wg_done/wg_fail vs timeout/no-tool-calls | D should be higher than A |

### 7.2 Secondary Metrics (Mechanism Understanding)

| Metric | Source | What it reveals |
|--------|--------|----------------|
| `verification_iterations` | adapter metadata | How many verify→fix cycles per trial |
| `self_termination_type` | adapter metadata | wg_done vs wg_fail vs max_turns vs timeout |
| `wg_tool_calls` | adapter metadata | Total wg tool usage |
| `wg_done` called | workgraph_state | Completion signaling compliance |
| `wg_log` entry count | workgraph_state | Progress journaling behavior |
| `wg_add` usage | workgraph_state | Decomposition behavior |
| Token usage | agent_result | Cost efficiency |
| Wall-clock seconds | trial timing | Real-time performance |

### 7.3 Analysis Plan

After the run:

1. **D vs A pass rate** — primary outcome. Use Fisher's exact test per task, McNemar's test for paired comparison across tasks.
2. **D vs A on hard tasks** — subset analysis on tasks where A pass rate < 50%.
3. **Verification loop effectiveness** — do trials with more verification iterations pass more often?
4. **Self-termination quality** — do wg_fail trials provide useful diagnostics vs A's silent failures?
5. **Turn distribution** — plot D's turn distribution. If bimodal (quick solves + long iterations), that validates the design.
6. **Cost-effectiveness** — pass rate per dollar. If D costs 1.35x but passes 1.3x more, it's a win.

---

## 8. Specific Adapter Code Changes

All changes are in `terminal-bench/wg/adapter.py`.

### 8.1 New Constants

```python
CONDITION_D_TOOLS = CONDITION_B_TOOLS  # Same 15 tools
```

### 8.2 New Prompt Builder

Add `build_condition_d_prompt()` as specified in Section 4.1 above.

### 8.3 Modified `WorkgraphAgent.__init__`

No changes needed — the base class already accepts `condition` as a string parameter.

### 8.4 Modified `WorkgraphAgent.setup()`

Change:
```python
if self.condition in ("B", "C"):
```
To:
```python
if self.condition in ("B", "C", "D"):
```

Then add D-specific setup after `wg init`:
```python
if self.condition == "D":
    # Bootstrap agency
    await _exec_wg_cmd_host(wg_dir, wg_bin, ["agency", "init"])
    # Create agent identity
    await _exec_wg_cmd_host(wg_dir, wg_bin, [
        "agent", "create", "solver",
        "--role", "programmer",
        "--tradeoff", "careful",
    ])
    # Assign to root task (done after root task creation in run())
    self._agent_identity = {
        "name": "solver",
        "role": "programmer",
        "tradeoff": "careful",
    }
```

### 8.5 Modified `WorkgraphAgent.run()`

Add D branch in the condition routing:
```python
elif self.condition == "D":
    tools = CONDITION_D_TOOLS
    root_task_id = f"tb-{uuid.uuid4().hex[:8]}"
    title = instruction[:100] + ("..." if len(instruction) > 100 else "")
    await _exec_wg_cmd_host(wg_dir, wg_bin, ["add", title, "--id", root_task_id])
    # Assign agent to task
    await _exec_wg_cmd_host(wg_dir, wg_bin, ["assign", root_task_id, "solver"])
    system_prompt = build_condition_d_prompt(
        instruction, root_task_id, self._agent_identity
    )
```

Add verification tracking in the tool execution loop:
```python
# Before the tool execution loop:
verification_count = 0
wg_tool_call_count = 0
termination_type = "max_turns"
verification_commands = []

# Inside the tool call loop:
if fn_name.startswith("wg_"):
    wg_tool_call_count += 1
if fn_name == "bash":
    cmd = fn_args.get("command", "")
    if any(kw in cmd.lower() for kw in ["test", "pytest", "make test", "cargo test",
                                          "npm test", "check", "verify", "./verify"]):
        verification_count += 1
        verification_commands.append(cmd[:200])

# After tool calls, check for termination signals:
if fn_name == "wg_done" and fn_args.get("task_id") == root_task_id:
    termination_type = "wg_done"
elif fn_name == "wg_fail" and fn_args.get("task_id") == root_task_id:
    termination_type = "wg_fail"

# After the loop, if no tool calls ended it:
if not message.tool_calls and termination_type == "max_turns":
    termination_type = "no_tool_calls"
```

### 8.6 New Class

```python
class ConditionDAgent(WorkgraphAgent):
    """Condition D: wg tools + autopoietic verification + agency + no turn cap."""

    @staticmethod
    def name() -> str:
        return "wg-condition-d"

    def __init__(self, *args, **kwargs):
        kwargs["condition"] = "D"
        kwargs.setdefault("max_turns", 200)
        super().__init__(*args, **kwargs)
```

### 8.7 Docstring Update

Update the module docstring to mention Condition D:
```python
"""
Terminal Bench Agent Adapter for Harbor Framework.

Supports four conditions:
  Condition A (control): bash + file tools only, no graph, no resume
  Condition B (treatment): full wg tool access, graph awareness, journal/resume
  Condition C (treatment): wg tools + skill injection + planning phase
  Condition D (treatment): wg tools + autopoietic verification + agency identity
"""
```

---

## 9. Risk Analysis

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| Agent ignores verification prompt (like B/C) | Medium | D's prompt is more prescriptive than C's. The "NEVER call wg_done without verification" is a stronger directive than C's gentle planning phase. If this still fails, the prompt isn't the issue — the model is. |
| 200-turn safety valve triggered frequently | Low | A's mean was 26 turns. Even with 3x verification iterations, typical trials should finish in ~80 turns. Monitor in smoke test. |
| Agency setup fails (starter data missing) | Low | `wg agency init` is tested. Verify in smoke test. If it fails, agency is non-critical — the prompt still works without it. |
| Cost blowup from uncapped turns | Medium | The $430 upper bound is acceptable. Monitor early trials — if mean cost/trial exceeds $1.50, pause and investigate. |
| wg binary not found or incompatible | Low | Same binary path as B/C. Already tested. |

---

## 10. Implementation Checklist

1. [ ] Add `build_condition_d_prompt()` to adapter.py
2. [ ] Add `CONDITION_D_TOOLS = CONDITION_B_TOOLS`
3. [ ] Add `ConditionDAgent` class
4. [ ] Modify `setup()` to handle condition "D" (agency init + agent create)
5. [ ] Modify `run()` to route condition "D" (prompt + assignment + tracking)
6. [ ] Add verification/termination tracking to the tool execution loop
7. [ ] Add enhanced metadata fields to `context.metadata`
8. [ ] Update module docstring
9. [ ] Smoke test with 1 trial on `constraints-scheduling`
10. [ ] Verify workgraph_state has agency data post-trial
11. [ ] Full run: 267 trials (89 tasks × 3)
12. [ ] Analysis: D vs A comparison

---

## Appendix: Experimental Conditions Summary

| | A (baseline) | B (cursed) | C (cursed) | **D (this design)** |
|---|---|---|---|---|
| Tools | 6 (bash+file) | 15 (+wg) | 15 (+wg) | **15 (+wg)** |
| Prompt style | Minimal | Tools listed | Skill injection + planning | **Autopoietic verification loop** |
| wg init | No | Yes | Yes | **Yes + agency bootstrap** |
| Agency | No | No | No | **Yes (role + tradeoff)** |
| Turn limit | 50 | 50 | 50 | **200 (safety valve only)** |
| Timeout | Default | Default | Default | **6x default (~30 min)** |
| Verification gate | No | No | No | **Yes — wg_done requires prior verification** |
| Failure protocol | Silent | wg_fail (rare) | wg_fail (sometimes) | **wg_fail with diagnostics after 3 stuck iterations** |
| Self-termination | Stop generating | wg_done (44%) | wg_done (86%) | **wg_done/wg_fail (target: 95%+)** |
