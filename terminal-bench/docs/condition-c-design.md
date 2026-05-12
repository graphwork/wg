# Condition C Design: wg Skill Injection + Planning Phase

**Status**: Draft
**Author**: tb-design-condition-c agent
**Date**: 2026-04-03

## 1. Problem Statement

Condition B gives the agent wg tools but the agent doesn't use them.
Calibration data (8 tasks, Qwen3-32B) shows the agent called `wg_done` on every task
but **never** used `wg_log`, `wg_add`, `wg_artifact`, or `wg_show`. The full benchmark
(267 trials, Minimax M2.7) shows Condition B scoring 16.2% mean reward vs Condition A's
20.3% — the overhead of wg tools *hurts* when the agent doesn't understand decomposition.

Condition C tests the hypothesis: **does teaching the agent *when* and *why* to decompose
(via skill injection) improve performance over both bare tools (A) and raw wg access (B)?**

## 2. Harbor Framework Prompting Rules & Constraints

### What Harbor allows

Harbor's `BaseAgent` protocol gives full control to the adapter:

| Mechanism | How it works | Source |
|-----------|-------------|--------|
| **System prompt** | Adapter builds `messages[0]` directly. No framework-imposed system message. | `adapter.py:build_condition_b_prompt()` |
| **Tools** | Adapter defines tool schemas as OpenAI function-calling dicts. No framework tool filtering. | `adapter.py:CONDITION_B_TOOLS` |
| **Tool execution** | Adapter handles all tool dispatch in `execute_tool()`. No framework sandbox constraints. | `adapter.py:execute_tool()` |
| **Prompt templates** | `BaseInstalledAgent` supports Jinja2 `prompt_template_path` via `render_instruction()`. Our adapter inherits from `BaseAgent` (not `BaseInstalledAgent`), so we control prompting directly. | `harbor/agents/installed/base.py`, `harbor/utils/templating.py` |
| **Agent kwargs** | Harbor's `--agent-kwargs` passes through to `__init__`. We can add `condition="C"` cleanly. | `harbor/cli/jobs.py:424` |
| **MCP servers / Skills dir** | `BaseAgent.__init__` accepts `mcp_servers` and `skills_dir`. Our adapter doesn't use these but could. | `harbor/agents/base.py:28-36` |

### What Harbor does NOT constrain

- No restriction on system prompt content or length
- No mandatory system message prefix
- No tool allowlisting — adapter defines its own tool schemas
- No restriction on multi-turn conversation structure
- No prohibition on "scaffolding" prompts (agent-specific reasoning strategies)

### TB benchmark rules (implicit)

Terminal Bench itself is a benchmark dataset — tasks provide `instruction`, `Dockerfile`,
`run-tests.sh`, and `task.yaml`. The benchmark doesn't constrain the agent's system
prompt. The only contract is: the agent receives an instruction string and must solve
the task inside the Docker environment. The verifier runs `run-tests.sh` and checks exit
code for `reward.txt`.

**Conclusion**: We have full latitude to inject skill prompts, add planning phases,
and modify the system message. The benchmark evaluates outcomes, not prompting strategy.

## 3. Condition C Design

### 3.1 Core Idea: Skill-Injected Decomposition

Condition C adds a **wg skill prompt** to the system message that teaches:
1. What wg is (external memory + task graph)
2. When to decompose vs solve directly (complexity heuristic)
3. How to use the tools effectively (patterns, not just syntax)
4. A mandatory planning phase before execution

### 3.2 Skill Prompt Segment

```
## wg Skill: Stigmergic Task Decomposition

You have access to a wg — a persistent task graph that survives context limits.
Use it strategically, not on every task.

### When to decompose (use wg_add)
- Task requires 3+ distinct phases (e.g., build → configure → test)
- Task involves multiple independent parts that could fail separately
- You anticipate running out of context before finishing
- You need to try multiple approaches and track which worked

### When to solve directly (skip wg_add)
- Task is a single clear action (fix a bug, write a file, run a command)
- You can see the complete solution path in < 10 steps
- The task doesn't require backtracking or exploration

### Tool patterns

**Progress logging** (ALWAYS do this):
```
wg_log("<task-id>", "Starting: <what you're about to do>")
# ... do work ...
wg_log("<task-id>", "Result: <what happened>")
```
This is your external memory. If your context fills up, a resumed agent sees these logs.

**Decomposition** (when task is complex):
```
wg_add("Phase 1: <title>", description="<what to do>")
wg_add("Phase 2: <title>", description="<what to do>", after="phase-1-title")
```
Then solve each phase, marking done as you go.

**Artifact recording** (when you create output files):
```
wg_artifact("<task-id>", "/path/to/output")
```

### Planning phase

Before writing any code or running commands, spend ONE turn analyzing the task:
1. What does the task ask for?
2. How many distinct steps are needed?
3. Should I decompose or solve directly?
4. What's my first action?

State your plan, then execute.
```

### 3.3 Planning Phase Design

The planning phase is implemented in the system prompt, not in code. The prompt instructs
the agent to spend its first turn on analysis before taking action. This is a "soft"
planning phase — the agent controls the loop, so we rely on instruction-following rather
than enforcing it mechanically.

**Why soft, not hard**: A hard planning phase (separate LLM call) would:
- Add latency and cost for simple tasks where planning is overhead
- Create an artificial boundary between planning and execution
- Require a separate prompt and parsing logic

The soft approach lets the agent skip planning for trivially simple tasks while still
using it for complex ones, matching the decomposition heuristic.

### 3.4 What Condition C Tests That B Doesn't

| Aspect | Condition B | Condition C |
|--------|------------|------------|
| Tool availability | wg tools available | Same wg tools available |
| Tool understanding | Tool names + brief descriptions | Skill prompt with heuristics, patterns, examples |
| Planning | None — agent dives in | Instructed to plan first |
| Logging | Agent ignores wg_log | Mandated "ALWAYS do this" |
| Decomposition guidance | "Use wg_add to decompose complex work" (1 line) | Decision heuristic: when to decompose vs solve directly |
| Overhead for simple tasks | Full wg prompt weight | Skill prompt weight, but heuristic says "skip wg_add" for simple tasks |

**The experimental variable is skill injection quality** — same tools, better instructions.

## 4. Model Selection

### Recommendation: Use Minimax M2.7 (same as A and B)

Rationale:
- **Apples-to-apples comparison** is the primary goal. Changing the model conflates
  two variables (skill injection + model capability).
- The question is: "Does skill injection help THIS model?" not "Does a better model
  with skill injection beat a worse model without it?"
- If Condition C with M2.7 shows improvement, that's a clean signal.
- If we later want to test model capability, that's a separate experiment axis.

**Future experiment**: A 2x2 design (model x condition) would test both variables, but
the immediate priority is isolating the skill injection effect.

## 5. State Snapshot Capture

### Per-trial wg state should be captured

The adapter already does this for Condition B:

```python
# adapter.py:808-814
if self.condition == "B" and wg_dir:
    wg_state_dst = self.logs_dir / "workgraph_state"
    shutil.copytree(wg_dir, str(wg_state_dst))
```

For Condition C, we capture the same artifacts plus additional analysis data:

| Artifact | Location | Purpose |
|----------|----------|---------|
| `workgraph_state/` | `logs_dir/workgraph_state/` | Full `.wg/` directory snapshot |
| `agent_loop.ndjson` | `logs_dir/agent_loop.ndjson` | LLM call log with tool calls |
| `planning_turn.json` | `logs_dir/planning_turn.json` | Extracted first-turn planning analysis |

### New: Planning turn extraction

After the run completes, extract the first assistant message and log it separately as
`planning_turn.json`. This enables analysis of:
- Did the agent actually plan? (vs diving straight into tools)
- Did it correctly classify task complexity?
- Did its plan match its actual execution?

Implementation: Add a post-run step in the adapter that reads `agent_loop.ndjson`,
finds the first `type: "turn"` entry, and writes it to `planning_turn.json`.

## 6. Adapter Modification Plan

### Files to change

#### `terminal-bench/wg/adapter.py`

1. **Add `build_condition_c_prompt()` function** (~line 554, after `build_condition_b_prompt`):
   - Start from Condition B's prompt structure
   - Replace the brief tool descriptions with the full skill prompt segment
   - Add the planning phase instruction
   - Keep the tool list identical to Condition B

2. **Add `CONDITION_C_TOOLS`** (alias for `CONDITION_B_TOOLS`):
   - Same tools as B — the variable is the prompt, not the tools
   - Alias makes the code self-documenting

3. **Modify `WorkgraphAgent.__init__`** (~line 621):
   - Accept `condition="C"` (currently only A/B)

4. **Modify `WorkgraphAgent.run`** (~line 663):
   - Add `elif self.condition == "C":` branch
   - Same setup as B (create root task, initialize wg) but use `build_condition_c_prompt`
   - Add post-run planning turn extraction

5. **Add `ConditionCAgent` class** (~line 845):
   ```python
   class ConditionCAgent(WorkgraphAgent):
       """Condition C: wg tools + skill injection + planning phase."""
       @staticmethod
       def name() -> str:
           return "wg-condition-c"
       def __init__(self, *args, **kwargs):
           kwargs["condition"] = "C"
           super().__init__(*args, **kwargs)
   ```

6. **Add `_extract_planning_turn()` method**:
   - Post-run, read `agent_loop.ndjson`
   - Find first assistant turn (type="turn", turn=0)
   - Write to `planning_turn.json`

#### `terminal-bench/tb-harness.sh`

7. **Add Condition C branch** (~line 148):
   - `elif [[ "$CONDITION" == "C" ]]; then`
   - Same wg initialization as B
   - Use the Condition C system prompt (skill injection version)
   - Same exec-mode as B ("full")

#### `terminal-bench/README.md`

8. **Update comparison table** to include Condition C
9. **Add Condition C section** describing skill injection

#### `terminal-bench/pyproject.toml`

10. **Add `wg-tb-condition-c` entry point** (optional, for convenience)

### What NOT to change

- Tool definitions (`CONDITION_B_TOOLS`) — C uses the same tools as B
- Tool execution handlers (`execute_tool`, `_exec_wg_cmd_host`) — unchanged
- Harbor BaseAgent interface — we already conform
- wg initialization logic — same as B

### Implementation order

1. Add `build_condition_c_prompt()` with the skill prompt segment
2. Add the condition C branch in `WorkgraphAgent.run()`
3. Add `ConditionCAgent` convenience class
4. Add planning turn extraction
5. Update `tb-harness.sh` for CLI usage
6. Update README and pyproject.toml
7. Run a smoke test with a simple task to verify the prompt renders correctly

## 7. Risks and Mitigations

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| Skill prompt too long, eating into context budget | Medium | Keep under 500 tokens. Measure actual token count. |
| Agent still ignores wg tools despite better instructions | Medium | Use "ALWAYS do this" framing for wg_log. Accept partial adoption. |
| Planning phase wastes a turn on simple tasks | Low | Heuristic says "skip for simple tasks." One wasted turn is ~30s. |
| Model can't follow multi-part instructions (M2.7 limitation) | Medium | If C doesn't improve over B, this suggests model capability is the bottleneck, which is itself a useful finding. |
| Condition C prompt leaks task-specific hints | Very Low | Skill prompt is task-agnostic. Review before deployment. |

## 8. Success Criteria

Condition C is a success if **any** of these hold (in order of importance):

1. **Higher mean reward** than Condition B (>16.2%) on the same task set
2. **Higher wg tool adoption**: wg_log usage in >50% of trials (vs 0% in B calibration)
3. **Planning turns observed**: First turn contains analysis, not tool calls, in >70% of trials
4. **Decomposition on complex tasks**: wg_add usage on hard tasks (vs 0% in B)

A null result (C ≈ B) is still informative — it suggests the bottleneck is model
capability, not instruction quality, which guides future work (try C with a stronger model).

## Appendix A: Draft System Prompt for Condition C

```
# Task Assignment

You are an AI agent completing a Terminal Bench task.
Your root task ID is: **{root_task_id}**

## wg: Your External Memory

You have a wg — a persistent task graph that acts as external memory.
It survives even if your context fills up. Use it.

### Always do this
- `wg_log("{root_task_id}", "Starting: <plan>")` before your first action
- `wg_log("{root_task_id}", "Done: <result>")` after completing a step
- `wg_done("{root_task_id}")` when the task is complete
- `wg_fail("{root_task_id}", "reason")` if you cannot complete the task

### Decompose when needed
If the task has 3+ distinct phases or might exhaust your context:
- `wg_add("Step 1: <title>")` to create subtasks
- Solve each subtask, then `wg_done` each one
- Finally `wg_done("{root_task_id}")`

If the task is simple (< 10 steps), skip decomposition and solve directly.

### Record outputs
- `wg_artifact("{root_task_id}", "/path/to/file")` for files you create

## Planning Phase

Before writing code or running commands, analyze the task in ONE response:
1. What does the task require?
2. How many steps? Simple (< 10) or complex (10+)?
3. Plan: decompose or solve directly?
4. First action?

Then execute your plan.

## Tools
- bash, read_file, write_file, edit_file, glob, grep — for working in the environment
- wg_log, wg_add, wg_done, wg_fail, wg_show, wg_list, wg_artifact, wg_msg_send, wg_msg_read — for task coordination

Begin by analyzing the task below, then execute.
```

## Appendix B: Token Budget Estimate

| Component | Est. tokens | Notes |
|-----------|------------|-------|
| Condition A system prompt | ~120 | Minimal |
| Condition B system prompt | ~280 | Tool list + brief wg instructions |
| Condition C system prompt | ~400 | Skill injection + planning phase |
| Condition C delta vs B | +120 | Modest overhead |
| Skill prompt segment | ~250 | Core of the injection |
| Planning instruction | ~80 | "Before writing code..." block |

The +120 token overhead is acceptable. At M2.7's rates, this adds negligible cost
per trial. The question is whether those 120 tokens produce better tool adoption.
