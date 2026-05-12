# Condition F Final Design: WG-Native with Full Context Parity

**Date:** 2026-04-04
**Task:** design-condition-f
**Role:** Architect (expensive, slow, verbose — thorough analysis)

---

## 0. Design Correction: Why F Must Keep WG

The previous F proposal (conditions-and-f-design.md §4.3, Decision 1) recommended dropping wg tools entirely, reasoning that A' (bare, 80%) outperforms D (wg + verify, 73%) and E (wg + org, 75%). This reasoning is wrong for two reasons:

1. **It abandons the thesis.** The entire experiment asks: "Does wg add value over bare agents?" Dropping wg from F means F can only match A', never prove wg's value. A positive F-without-wg result would be evidence *against* wg.

2. **It misattributes E's failure.** E failed not because wg is bad, but because M2.7 was never taught how to use wg. Zero `--after` dependencies across 30 E trials. 80% of B agents ignored wg tools entirely. The model gravitates away from wg because the system prompt doesn't teach it effectively — Claude agents succeed with wg because CLAUDE.md and MEMORY.md provide operational patterns that M2.7 never receives.

**F's design principle:** Close the context gap. Give M2.7 the same wg operational knowledge that makes Claude agents effective, then measure whether wg adds value.

---

## 1. WG Context Injection (~1,100 tokens)

This is the distilled "kernel of total utility" from CLAUDE.md and MEMORY.md. It teaches M2.7 the operational patterns that make Claude agents effective wg users.

```
## wg Quick Guide

wg (wg) is a persistent task coordination graph. Tasks have
dependencies, statuses, and verification gates. It acts as your external
memory — if your context fills up, your wg_log entries survive.

### Task Lifecycle
open → in-progress → done/failed

### Creating Tasks with Dependencies
Use `after` to express dependency order:
  wg_add("Build library", after="clone-repo")
  wg_add("Run tests", after="build-library")

CRITICAL: Always use `after` for dependent steps. NEVER create flat task
lists — every step that depends on a previous step MUST declare the edge.

### Verification Gates
Use `verify` to attach a machine-checkable pass/fail gate:
  wg_add("Compile extension", verify="python -c 'import myext' exits 0")
Tasks with verify must pass the check before they can be marked done.

### When to Decompose
- 3+ genuinely independent phases → create subtasks with wg_add + after edges
- Single file, single function, single config → solve directly, no decomposition
- Golden rule: same files = sequential edges (NEVER parallelize tasks on same files)
- If in doubt, don't decompose — overhead on atomic tasks hurts more than it helps

### Progress & Termination
- wg_log("{id}", "message") — journal entry (external memory, persists across restarts)
- wg_done("{id}") — task complete (ONLY after verification passes)
- wg_fail("{id}", "reason") — cannot complete (include what you tried and what failed)
- wg_artifact("{id}", "/path") — record output files

### Dependency Patterns
- Pipeline: A → B → C (each wg_add uses after= pointing to previous)
- Fan-out/in: A → [B, C] → D (B and C have after=A; D has after=B,C)

### Example: Multi-Step Build Task
  wg_add("Clone and patch source")                         → auto-ID: clone-and-patch-source
  wg_add("Compile extension", after="clone-and-patch-source",
         verify="python -c 'import ext' exits 0")          → auto-ID: compile-extension
  wg_add("Run test suite", after="compile-extension",
         verify="pytest /tests/ -v exits 0")                → auto-ID: run-test-suite
Then implement each step in order, wg_done each, wg_done root task last.

### Example: Atomic Task (single function/config)
Don't decompose. Implement directly, verify empirically, then wg_done.
```

**Token count estimate:** ~1,100 tokens (280 words). Well within the 1-2K target.

**What's included:** `--after` with examples, `--verify` with examples, decomposition heuristic, dependency patterns, lifecycle, all tool usage patterns.

**What's excluded:** Cycles/`--max-iterations` (irrelevant in single-agent Harbor), orchestrator role description (irrelevant), federation, agency system, service configuration, development instructions.

---

## 2. Complete F Prompt Template

```python
def build_condition_f_prompt(instruction: str, root_task_id: str) -> str:
    """Condition F: wg-native agent with full context parity.

    Design principles:
    - Keep wg tools (proves wg adds value over bare agents)
    - Inject distilled wg knowledge (closes context gap with Claude agents)
    - Empirical verification first (test discovery before self-assessment)
    - Adaptive decomposition (multi-step = decompose; atomic = direct)
    - Time-aware termination (clean failure instead of timeout)
    """
    return (
        "You are a coding agent completing a Terminal Bench task.\n"
        f"Your root task ID is: **{root_task_id}**\n\n"

        # --- WG Context Injection (distilled from CLAUDE.md + MEMORY.md) ---
        "## wg Quick Guide\n\n"
        "wg (wg) is a persistent task coordination graph. Tasks have "
        "dependencies, statuses, and verification gates. It acts as your external "
        "memory — if your context fills up, your wg_log entries survive.\n\n"

        "### Creating Tasks with Dependencies\n"
        "Use `after` to express dependency order:\n"
        "  `wg_add(\"Build library\", after=\"clone-repo\")`\n"
        "  `wg_add(\"Run tests\", after=\"build-library\")`\n\n"
        "CRITICAL: Always use `after` for dependent steps. NEVER create flat task "
        "lists — every step that depends on a previous step MUST declare the edge.\n\n"

        "### Verification Gates\n"
        "Use `verify` to attach a machine-checkable pass/fail gate:\n"
        "  `wg_add(\"Compile ext\", verify=\"python -c 'import myext'\")`\n"
        "Tasks with verify must pass the check before they can be marked done.\n\n"

        "### When to Decompose\n"
        "- 3+ genuinely independent phases → create subtasks with wg_add + after\n"
        "- Single file/function/config → solve directly, no decomposition\n"
        "- Same files = sequential edges (never parallelize)\n"
        "- If in doubt, don't decompose\n\n"

        "### Progress & Termination\n"
        f"- `wg_log(\"{root_task_id}\", \"message\")` — journal progress\n"
        f"- `wg_done(\"{root_task_id}\")` — complete (ONLY after verification passes)\n"
        f"- `wg_fail(\"{root_task_id}\", \"reason\")` — cannot complete (with diagnostics)\n"
        f"- `wg_artifact(\"{root_task_id}\", \"/path\")` — record output files\n\n"

        # --- Core Protocol ---
        "## Strategy: Discover → Plan → Implement → Verify → Iterate\n\n"

        "### Step 1: Discover Tests\n"
        "Before writing ANY code, find the task's existing test suite:\n"
        "```\n"
        "bash(\"find /tests -name 'test_*.py' -o -name '*_test.py' 2>/dev/null\")\n"
        "bash(\"ls /tests/ 2>/dev/null\")\n"
        "```\n"
        "Read any test files you find. They define what 'correct' means.\n"
        "The external verifier runs exactly these tests to score you.\n"
        "Understanding them FIRST tells you exactly what to build.\n\n"

        "### Step 2: Classify & Plan\n"
        "- **ATOMIC** (single file, single function, single config): "
        "Implement directly. Do NOT decompose.\n"
        "- **MULTI-STEP** (multiple files, build pipeline, system setup): "
        "Create subtasks with `wg_add` and `after` dependency edges.\n"
        f"Log your plan: `wg_log(\"{root_task_id}\", \"Plan: <classification> — <steps>\")`\n\n"

        "### Step 3: Implement\n"
        "Write your solution. For multi-step tasks, implement each subtask "
        "in dependency order, marking each done with `wg_done`.\n\n"

        "### Step 4: Verify Empirically\n"
        "Run the discovered test files:\n"
        "```\n"
        "bash(\"cd /tests && python -m pytest test_outputs.py -v 2>&1 | tail -80\")\n"
        "```\n"
        "If no test files were found, verify by running the code and checking outputs.\n\n"
        "**CRITICAL**: Existing test results are AUTHORITATIVE.\n"
        "If an existing test FAILS, that is ground truth — do NOT declare success.\n"
        "Your own ad-hoc tests supplement existing tests, never override them.\n"
        f"Log the result: `wg_log(\"{root_task_id}\", \"VERIFY: <PASS|FAIL> — <evidence>\")`\n\n"

        "### Step 5: Iterate on Failures\n"
        "If tests fail:\n"
        "1. Read the failure output — it tells you exactly what's wrong.\n"
        "2. Diagnose the root cause (not just the symptom).\n"
        "3. Fix it.\n"
        "4. Re-run the tests.\n"
        "5. Repeat up to 5 times. If trying the same fix twice, "
        "step back and reconsider entirely.\n\n"

        "### Step 6: Declare\n"
        f"- Verification passes: `wg_done(\"{root_task_id}\")`\n"
        f"- Stuck after 5 iterations: "
        f"`wg_fail(\"{root_task_id}\", \"reason: <what failed and what was tried>\")`\n\n"

        "## Time Management\n"
        "You have 30 minutes maximum. Budget roughly:\n"
        "- Test discovery + planning: 2 minutes\n"
        "- Implementation: 15 minutes\n"
        "- Verification + iteration: 10 minutes\n"
        "- If stuck for 20+ minutes with no progress, call `wg_fail` immediately.\n\n"

        "## Tools\n"
        "- `bash` — Run commands, install packages, compile, test\n"
        "- `read_file`, `write_file`, `edit_file` — File operations\n"
        "- `glob`, `grep` — Search the codebase\n"
        "- `wg_log`, `wg_add`, `wg_done`, `wg_fail` — Task coordination (see Quick Guide)\n"
        "- `wg_show`, `wg_list` — Inspect task graph\n"
        "- `wg_artifact`, `wg_msg_send`, `wg_msg_read` — Record artifacts, communicate\n\n"

        "Begin by discovering tests, then plan your approach.\n"
    )
```

**Estimated system prompt size:** ~2,100 tokens (530 words). Compare:
- A: ~200 tokens
- D: ~800 tokens
- E: ~1,200 tokens
- F: ~2,100 tokens (1,100 wg guide + 1,000 protocol)

The extra 900 tokens over E are the wg context injection. This is the core thesis: those tokens buy the agent the same wg competence that Claude agents get from CLAUDE.md.

---

## 3. Tool Set Specification

### 3.1 Current Tool Inventory

| Tool | A/A' | B/C | D/E | F |
|------|------|-----|-----|---|
| bash | Yes | Yes | Yes | Yes |
| read_file | Yes | Yes | Yes | Yes |
| write_file | Yes | Yes | Yes | Yes |
| edit_file | Yes | Yes | Yes | Yes |
| glob | Yes | Yes | Yes | Yes |
| grep | Yes | Yes | Yes | Yes |
| wg_show | — | Yes | Yes | Yes |
| wg_list | — | Yes | Yes | Yes |
| wg_add | — | Yes | Yes | **Yes + verify** |
| wg_done | — | Yes | Yes | Yes |
| wg_fail | — | Yes | Yes | Yes |
| wg_log | — | Yes | Yes | Yes |
| wg_artifact | — | Yes | Yes | Yes |
| wg_msg_send | — | Yes | Yes | Yes |
| wg_msg_read | — | Yes | Yes | Yes |
| **Total** | **6** | **15** | **15** | **15** |

### 3.2 Tool Gap: Claude Agents vs TB Agents

| Capability | Claude agents (this repo) | TB agents (Harbor) | Gap | Remediation |
|-----------|--------------------------|-------------------|-----|-------------|
| **CLAUDE.md context** | Auto-loaded by Claude Code harness | Not injected | **CRITICAL** | F injects distilled wg guide in system prompt (§1) |
| **MEMORY.md context** | Auto-loaded | Not injected | **HIGH** | F injects key patterns from MEMORY.md into wg guide |
| **`--verify` on wg add** | Available via CLI | **Missing from wg_add tool schema** | **HIGH** | Add `verify` parameter to WG_ADD_TOOL (§3.3) |
| **`--id` on wg add** | Available via CLI | Missing from tool schema | MEDIUM | Add `id` parameter — lets agents control task IDs for reliable `after` references |
| **Web search/fetch** | Available (WebSearch, WebFetch tools) | Not available | LOW | bash + curl provides basic fetch; structured search would need API key injection |
| **wg CLI directly** | Full CLI access | Function-calling wrapper (subset) | LOW | The 9 wg tools cover the essential operations |
| **File system awareness** | Reads project structure, knows conventions | No project context | LOW | Not relevant for TB tasks (self-contained containers) |

### 3.3 Required Tool Changes for F

#### Change 1: Add `verify` parameter to WG_ADD_TOOL (HIGH priority)

The `--verify` flag is the mechanism CLAUDE.md emphasizes for machine-checkable verification gates. Without it, the F prompt teaches a concept the agent cannot use.

**Tool schema change:**
```python
WG_ADD_TOOL_F = {
    "type": "function",
    "function": {
        "name": "wg_add",
        "description": "Create a new task in the wg.",
        "parameters": {
            "type": "object",
            "required": ["title"],
            "properties": {
                "title": {"type": "string", "description": "Task title."},
                "after": {
                    "type": "string",
                    "description": "Comma-separated dependency task IDs.",
                },
                "description": {
                    "type": "string",
                    "description": "Detailed description.",
                },
                "verify": {
                    "type": "string",
                    "description": (
                        "Machine-checkable verification command. "
                        "Task cannot be marked done until this command exits 0. "
                        "Example: \"python -m pytest /tests/test_outputs.py -v\""
                    ),
                },
                "id": {
                    "type": "string",
                    "description": (
                        "Explicit task ID (kebab-case). If omitted, auto-generated from title. "
                        "Use this to create predictable IDs for after references."
                    ),
                },
            },
        },
    },
}
```

**Execute handler change:**
```python
elif tool_name == "wg_add":
    cmd = ["add", args["title"]]
    if args.get("id"):
        cmd += ["--id", args["id"]]
    if args.get("after"):
        cmd += ["--after", args["after"]]
    if args.get("description"):
        cmd += ["-d", args["description"]]
    if args.get("verify"):
        cmd += ["--verify", args["verify"]]
    return await _exec_wg_cmd_host(wg_dir, wg_bin, cmd)
```

#### Change 2: Web fetch via bash (NO tool change needed)

The bash tool already provides `curl` and `wget` inside Harbor containers. No additional tool is needed. If a task requires downloading dependencies or fetching data, the agent can:
```
bash("curl -sL https://example.com/data.tar.gz -o /tmp/data.tar.gz")
```

This provides functional parity with Claude's WebFetch for the operations TB tasks actually require (downloading packages, fetching resources). Structured web *search* (Google, etc.) is not needed for TB tasks and would require API key injection into containers.

### 3.4 F Tool Set Definition

```python
# F uses enhanced wg tools (with verify + id on wg_add)
CONDITION_F_TOOLS = [
    BASH_TOOL,
    READ_FILE_TOOL,
    WRITE_FILE_TOOL,
    EDIT_FILE_TOOL,
    GLOB_TOOL,
    GREP_TOOL,
    WG_SHOW_TOOL,
    WG_LIST_TOOL,
    WG_ADD_TOOL_F,   # Enhanced with verify + id parameters
    WG_DONE_TOOL,
    WG_FAIL_TOOL,
    WG_LOG_TOOL,
    WG_ARTIFACT_TOOL,
    WG_MSG_SEND_TOOL,
    WG_MSG_READ_TOOL,
]
```

---

## 4. Adapter Implementation

### 4.1 ConditionFAgent Class

```python
class ConditionFAgent(WorkgraphAgent):
    """Condition F: wg-native agent with full context parity.

    Key differences from E:
    - Distilled wg context injection (from CLAUDE.md + MEMORY.md)
    - Empirical verification first (test discovery before implementation)
    - Adaptive decomposition (no forced orchestrator framing)
    - Enhanced wg_add with --verify and --id parameters
    - Time-aware termination
    - Compliant turn limit (1M, not 300)
    """

    @staticmethod
    def name() -> str:
        return "wg-condition-f"

    def __init__(self, *args, **kwargs):
        kwargs["condition"] = "F"
        kwargs["model_name"] = BENCHMARK_MODEL
        kwargs.setdefault("max_turns", 1000000)  # TB2 compliance: no turn cap
        super().__init__(*args, **kwargs)
```

### 4.2 Changes to WorkgraphAgent

#### In `__init__` validation:
Add `"F"` to accepted conditions (wherever condition is validated).

#### In `setup()`:
F uses wg but does NOT bootstrap agency. No orchestrator/solver identity — F is a coding agent, not a role-playing one.

```python
# In setup():
if self.condition in ("B", "C", "D", "E", "F"):
    # ... existing wg init code ...

    if self.condition == "F":
        # No agency bootstrap — F is a plain coding agent with wg tools
        logger.info("Condition F: wg initialized, no agency bootstrap")
```

#### In `run()`:
Route condition F to use `CONDITION_F_TOOLS`, create root task, and call `build_condition_f_prompt`.

```python
# In run(), add before the elif chain or integrate:
elif self.condition == "F":
    tools = CONDITION_F_TOOLS
    root_task_id = f"tb-{uuid.uuid4().hex[:8]}"
    title = instruction[:100] + ("..." if len(instruction) > 100 else "")
    await _exec_wg_cmd_host(wg_dir, wg_bin, ["add", title, "--id", root_task_id])
    system_prompt = build_condition_f_prompt(instruction, root_task_id)
```

#### In `execute_tool()`:
Handle the enhanced `wg_add` with `verify` and `id` parameters (see §3.3).

#### In termination logic:
F uses the same `done_or_failed` termination as D/E — stop loop when agent calls `wg_done` or `wg_fail` on the root task.

```python
# Extend the D/E termination check:
if self.condition in ("D", "E", "F") and done_or_failed:
    ...
```

#### In metadata collection:
Track F-specific metrics:

```python
if self.condition == "F":
    metadata.update({
        "verification_iterations": trial_log.verification_count,
        "self_termination_type": trial_log.termination_type,
        "wg_tool_calls": sum(trial_log.wg_command_counts.values()),
        "verification_commands": trial_log.verification_commands,
        "decomposition_task_count": len(trial_log.decomposition_tasks),
        "decomposition_tasks": trial_log.decomposition_tasks[:20],
        "test_discovery": True,  # Flag for analysis scripts
        "after_usage_count": 0,  # TODO: count --after params in wg_add calls
        "verify_usage_count": 0,  # TODO: count --verify params in wg_add calls
    })
```

### 4.3 Module Docstring Update

```python
"""
Terminal Bench Agent Adapter for Harbor Framework.

Bridges Harbor's agent protocol to the wg native executor concept.
Supports six conditions:
  Condition A (control): bash + file tools only, no graph, no resume
  Condition B (treatment): full wg tool access, graph awareness, journal/resume
  Condition C (treatment): wg tools + skill injection + planning phase
  Condition D (treatment): wg tools + autopoietic verification + agency identity
  Condition E (treatment): wg tools + organization generation + independent verification
  Condition F (treatment): wg tools + distilled context injection + empirical verification
"""
```

---

## 5. Run Configuration

### 5.1 TB2-Compliant Harbor Command

The TB2 compliance audit (research-audit-tb2) found two violations that F must correct:

1. **Turn cap**: Must be uncapped (reference agent uses 1,000,000)
2. **Timeout**: Must use per-task defaults from task.toml, not a flat 30-min override

```bash
harbor run \
    -d terminal-bench/terminal-bench-2 \
    --agent-import-path "wg.adapter:ConditionFAgent" \
    -m "openrouter/minimax/minimax-m2.7" \
    -k 3 \
    -n 4 \
    --job-name condition-f-pilot \
    --jobs-dir terminal-bench/results/condition-f-pilot \
    --no-delete \
    --debug \
    --ak "max_turns=1000000" \
    --ak "temperature=0.0" \
    --agent-timeout-multiplier 1.0 \
    -y \
    2>&1 | tee terminal-bench/results/condition-f-pilot/run.log
```

**Key changes from previous F proposal:**
- `max_turns=1000000` (was 200) — TB2 compliance
- No `--timeout` flag — let Harbor use per-task task.toml defaults
- `--agent-timeout-multiplier 1.0` — TB2 compliance (explicit)

### 5.2 Pilot Configuration

Run on the same 10 pilot tasks used for A'/D/E:

| # | Task | Category | Why included |
|---|------|----------|-------------|
| 1 | build-cython-ext | multi-step build | D failed (33%), E passed (100%) — tests wg decomposition |
| 2 | cancel-async-tasks | single function | E failed (0%) — tests adaptive non-decomposition + test discovery |
| 3 | nginx-request-logging | system config | D/E both 100% — baseline multi-step |
| 4 | overfull-hbox | LaTeX debugging | All conditions weak — hard to verify |
| 5 | regex-log | text processing | E failed catastrophically (0%) — tests atomic classification |
| 6 | count-dataset-tokens | data processing | All conditions 100% — easy baseline |
| 7 | custom-memory-heap-crash | C debugging | A'/D 100% — moderate baseline |
| 8 | merge-diff-arc-agi-task | multi-step reasoning | E excelled (100%) — tests decomposition benefit |
| 9 | qemu-startup | system emulation | A' 100%, E 100% — moderate |
| 10 | sparql-university | query language | A' 100%, D/E 67% — tests empirical verification |

3 trials per task = 30 pilot trials.

---

## 6. Success Criteria and Hypotheses

### 6.1 Primary Hypothesis

**H1: F (wg-native + context parity) > A' (bare agent) in pass rate.**

If true: wg adds value when the agent knows how to use it. The context gap was the problem, not wg itself.

If false: wg genuinely doesn't help in single-agent settings, even with proper instruction.

### 6.2 Secondary Hypotheses

**H2: F's `--after` usage rate > 50% on multi-step tasks.**

E achieved 0% `--after` usage. If F's wg context injection works, agents should actually build dependency graphs. This is the proximal mechanism — if agents don't use `--after`, the context injection failed.

**H3: F's false-PASS rate < E's 100% on failures.**

E never caught a bug during verification. F's test discovery should catch failures before `wg_done`. Target: <30% false-PASS rate.

**H4: F outperforms A' specifically on multi-step tasks.**

A' achieves 100% on build-cython-ext but only 67% on cancel-async-tasks. F should match A' on atomic tasks (no decomposition overhead) and beat A' on multi-step tasks (decomposition + verification gates).

### 6.3 Success Criteria (Pilot)

| Criterion | Threshold | Rationale |
|-----------|-----------|-----------|
| Overall pass rate | > 80% | Must exceed A' to validate thesis |
| Multi-step task pass rate | > 90% | Where wg should shine |
| Atomic task pass rate | ≥ 67% | Must not regress below A' |
| `--after` dependency usage | > 50% of multi-step trials | Proves context injection works |
| False-PASS rate | < 50% | Test discovery catches most bugs |
| Clean termination (wg_done/wg_fail) | > 90% | wg provides structured signaling |
| Mean tokens/trial | < 800K | Must not blow up like E's failures |

### 6.4 Predicted Per-Task Outcomes

| Task | A' | D | E | F (predicted) | F mechanism |
|------|-----|---|---|---------------|-------------|
| build-cython-ext | 100% | 33% | 100% | **100%** | Decomposition + test discovery catches what D missed |
| cancel-async-tasks | 67% | 67% | 0% | **67-100%** | Atomic classification (no decomposition overhead) + /tests/test_outputs.py reveals edge case |
| nginx-request-logging | 67% | 100% | 100% | **100%** | Decomposition + verification (like D/E) |
| overfull-hbox | 33% | 33% | 67% | **33-67%** | Test files may not exist; falls back to empirical verification |
| regex-log | 67% | 67% | 0% | **67%** | Atomic classification prevents fragmentation; test file reveals exact expected matches |
| count-dataset-tokens | 100% | 100% | 100% | **100%** | Easy task, all conditions pass |
| custom-memory-heap-crash | 100% | 100% | 100% | **100%** | Easy task, all conditions pass |
| merge-diff-arc-agi-task | 67% | 67% | 100% | **100%** | Decomposition with proper --after edges (like E, but structured) |
| qemu-startup | 100% | 100% | 100% | **100%** | Easy task |
| sparql-university | 100% | 67% | 67% | **100%** | Test file shows expected query results; agent can verify empirically |

**Predicted pilot pass rate: 83-93% (25-28/30).**

---

## 7. Cost Estimate

### 7.1 Token Budget

| Component | Tokens |
|-----------|--------|
| System prompt | ~2,100 |
| Task instruction (avg) | ~500 |
| Per-turn overhead (tool schemas) | ~3,000 (15 tools × ~200 tokens each) |
| WG context injection delta vs E | +900 tokens |

The wg context injection adds ~900 tokens to the system prompt vs E. Over a 30-turn trial, this adds ~27K tokens to the total (900 × 30 turns of carrying the system prompt in context). Negligible compared to the 521K-821K average trial cost.

### 7.2 Cost Projections

| Scenario | Tokens/trial | Cost (30 pilot) | Cost (267 full) |
|----------|-------------|-----------------|-----------------|
| Optimistic | ~500K (D-like efficiency + structured termination) | $43 | $385 |
| Expected | ~650K (between D and A', test iteration adds turns) | $56 | $500 |
| Conservative | ~850K (A'-like, test-iterate loops on hard tasks) | $74 | $654 |

**Time estimate**: With 4 concurrent trials and variable per-task timeouts (TB2 compliant), the 30-trial pilot takes ~1-2 hours. Full 267-trial run takes ~9-15 hours.

---

## 8. Design Comparison: F vs All Conditions

| Dimension | A' | D | E | **F** |
|-----------|-----|---|---|-------|
| Tools | 6 | 15 | 15 | **15 (enhanced wg_add)** |
| System prompt tokens | ~200 | ~800 | ~1,200 | **~2,100** |
| WG context injection | None | None | None | **Yes (~1,100 tokens)** |
| Turn limit | 9999 | 200 | 300 | **1,000,000 (TB2 compliant)** |
| Per-task timeout | 30min flat | 30min flat | 30min flat | **Per task.toml (TB2 compliant)** |
| Test discovery | No | No | No | **Yes — explicit Step 1** |
| Verification gate | No | Self-test | Self-test (always PASS) | **Existing tests authoritative** |
| `--after` dependency teaching | N/A | Brief mention | Brief mention | **Examples + CRITICAL emphasis** |
| `--verify` gate teaching | N/A | Not available | Not available | **Available + taught** |
| Decomposition | No | Optional | Mandatory | **Adaptive (classify first)** |
| Agency identity | No | solver/careful | orchestrator/thorough | **No (plain coding agent)** |
| Failure protocol | Silent stop | wg_fail after 3 iters | wg_fail after 6 iters | **wg_fail after 5 iters or 20min** |
| Time awareness | No | No | No | **Yes — explicit 30-min budget** |
| Expected cost/trial | 821K | 521K | 683K | **~650K** |

### What F takes from each condition

| Source | Adopted | Rejected |
|--------|---------|----------|
| **A'** | No turn cap; direct agent framing; no forced decomposition | Silent failures; no termination signaling; no verification |
| **D** | Verification loop (attempt→verify→iterate→declare); wg_done gated on verification; cost-efficient convergence | Self-authored verification only; agency identity overhead |
| **E** | Decomposition for multi-step tasks; structured phase protocol | Mandatory decomposition; orchestrator framing; "independent" verification theater; flat subtask lists |
| **CLAUDE.md** | `--after` patterns; `--verify` gates; decomposition heuristic; TDD principle | Orchestrator role; cycle support; service configuration |
| **condition-e-improvement.md** | Test discovery (#1); empirical verification (#3); time-aware bailout (#4); remove orchestrator framing (#6) | — (all top improvements adopted) |
| **TB2 audit** | Uncapped turns; per-task timeouts; timeout_multiplier=1.0 | — (all compliance fixes adopted) |

---

## 9. Implementation Checklist

### Adapter Changes (adapter.py)

1. [ ] Add `WG_ADD_TOOL_F` with `verify` and `id` parameters (§3.3)
2. [ ] Add `CONDITION_F_TOOLS` list using enhanced wg_add (§3.4)
3. [ ] Add `build_condition_f_prompt()` function (§2)
4. [ ] Add `ConditionFAgent` class (§4.1)
5. [ ] Update `setup()` to handle condition "F" (wg init, no agency) (§4.2)
6. [ ] Update `run()` to route condition "F" (§4.2)
7. [ ] Update `execute_tool()` to handle `verify` and `id` params in wg_add (§3.3)
8. [ ] Update termination logic to include "F" in D/E pattern (§4.2)
9. [ ] Update metadata collection for condition F (§4.2)
10. [ ] Update module docstring (§4.3)

### Compliance Fixes (reproduce.sh / tb-harness.sh)

11. [ ] Set `MAX_TURNS=1000000` in reproduce.sh
12. [ ] Remove `--timeout "$TIMEOUT"` from `harbor run` in reproduce.sh (use task.toml)
13. [ ] Set `MAX_TURNS=1000000` in tb-harness.sh
14. [ ] Remove timeout override in tb-harness.sh

### Verification & Analysis

15. [ ] Smoke test: 1 trial on cancel-async-tasks (test discovery should make the biggest difference)
16. [ ] Pilot run: 10 tasks × 3 trials
17. [ ] Measure `--after` usage rate (key proxy for context injection effectiveness)
18. [ ] Measure false-PASS rate (key proxy for test discovery effectiveness)
19. [ ] Compare F pilot to A'/D/E pilot results
20. [ ] If F > A': full 89-task run (267 trials)

---

## 10. Risk Analysis

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| M2.7 ignores wg context injection (like B's 80% tool ignorance) | MEDIUM | HIGH | The injection is much more detailed than B's brief guidelines; includes concrete examples with pre-filled task IDs. If M2.7 still ignores it, the model simply can't use wg. |
| Larger system prompt degrades model performance | LOW | MEDIUM | The extra ~900 tokens are well within M2.7's capabilities. E's ~1,200 token prompt didn't degrade performance; F's ~2,100 shouldn't either. |
| `--verify` gates block task completion on false negatives | LOW | LOW | The verify command runs in the container; if it fails incorrectly, the agent can retry or use wg_fail. |
| Per-task timeouts are too short for some tasks | LOW | MEDIUM | TB2 compliance requires per-task timeouts. If some tasks have tight timeouts, that's the benchmark's design — same constraint applies to all competitors. |
| Uncapped turns cause runaway token consumption | MEDIUM | MEDIUM | F's time-aware bailout and wg_fail protocol should prevent runaway. Monitor cost during pilot. |

---

## Appendix A: Why Not Drop WG (Detailed Argument)

The previous F proposal argued (§5.1-5.4) that wg is overhead in single-agent Harbor trials based on:
- A' (no wg) = 80% > D (wg) = 73% > B (wg, passive) = 38%
- 80% of B agents ignore wg tools
- E uses 5× more wg calls than D for no benefit
- Zero `--after` usage means wg's dependency model is unused
- No wg service running (no coordinator, no parallel agents)

This analysis is correct about the *current* state but draws the wrong conclusion. The evidence shows that **untaught wg usage is overhead** — it does not show that wg is inherently overhead.

Consider an analogy: if you give a new developer git without teaching them branching, they'll use `git add -A && git commit` as a save button. The lack of branching usage doesn't prove git branches are overhead — it proves the developer needs training.

E's zero `--after` usage is the smoking gun. The agent creates flat subtask lists because the prompt says "create tasks for each step using wg_add" without teaching that steps with dependencies need `--after` edges. CLAUDE.md's first principle is "use `--after` for dependency edges." Without this instruction, the agent can't use wg's dependency model.

F tests whether the context gap is the bottleneck. If F achieves >80% with wg, it validates the thesis. If F achieves ≤80% despite proper wg teaching, *then* we have evidence that wg is overhead in single-agent trials — and only then should we consider dropping it.

## Appendix B: Token Estimate for WG Context Injection

Counted using cl100k_base tokenizer approximation (4 chars/token average):

| Section | Words | Est. Tokens |
|---------|-------|-------------|
| Quick Guide header + lifecycle | 35 | 50 |
| Creating Tasks with Dependencies | 65 | 95 |
| Verification Gates | 40 | 60 |
| When to Decompose | 50 | 75 |
| Progress & Termination | 55 | 80 |
| Dependency Patterns | 25 | 40 |
| Multi-Step Example | 80 | 140 |
| Atomic Example | 15 | 25 |
| **Total WG Guide** | **365** | **~565** |

In the prompt template (which includes the guide inline plus protocol):
| Section | Est. Tokens |
|---------|-------------|
| WG Quick Guide (inline) | ~565 |
| Strategy protocol (Steps 1-6) | ~900 |
| Time management | ~120 |
| Tool list | ~200 |
| Framing + root task ID | ~100 |
| **Total F system prompt** | **~1,885** |

Actual token count will vary by tokenizer. The estimate of ~2,100 in §2 accounts for formatting overhead and special tokens.
