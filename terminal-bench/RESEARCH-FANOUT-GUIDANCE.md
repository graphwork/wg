# Research: Agent Fanout Guidance and TB Prompt Injection

**Task:** tb-research-fanout
**Date:** 2026-04-14

---

## 1. Agent System Prompt / Context Injection Architecture

### 1.1 Prompt Assembly Entry Point

**File:** `src/service/executor.rs:762` — `build_prompt()`

This is the main function that assembles the complete agent prompt. It takes `TemplateVars`, a `ContextScope` (clean/task/graph/full), and a `ScopeContext`, then concatenates sections in a defined order:

1. System awareness preamble (full scope only)
2. Skills preamble
3. Task assignment header
4. Agent identity (from agency system)
5. Task details (ID, title, description)
6. Pattern keywords glossary (conditional)
7. Verification criteria
8. Discovered test files (task+ scope)
9. Tags/skills info (task+ scope)
10. Context from dependencies
11. Triage mode (if failed deps)
12. Previous attempt context (on retry)
13. Queued messages (task+ scope)
14. Downstream awareness (task+ scope)
15. Loop info
16. WG usage guide (task+ scope, non-Claude models)
17. **REQUIRED_WORKFLOW_SECTION** (task+ scope)
18. **GIT_HYGIENE_SECTION** (task+ scope)
19. **MESSAGE_POLLING_SECTION** (task+ scope)
20. Telegram escalation (task+ scope, if configured)
21. **ETHOS_SECTION** ("The Graph is Alive") (task+ scope)
22. **DECOMPOSITION GUIDANCE** (task+ scope — adaptive or static)
23. **RESEARCH_HINTS_SECTION** (task+ scope)
24. **GRAPH_PATTERNS_SECTION** (task+ scope)
25. **REUSABLE_FUNCTIONS_SECTION** (task+ scope)
26. **CRITICAL_WG_CLI_SECTION** (task+ scope)
27. Additional context (task+ scope)
28. Graph summary / CLAUDE.md (graph/full scope)

### 1.2 Tiered Knowledge Guide (Non-Claude Models)

**File:** `src/commands/spawn/context.rs:644-815` — `build_tiered_guide()`

Three tiers based on model context window:
- **Essential** (8KB, line 667): Core agent guide with decision framework
- **Core** (16KB, line 792): Essential + communication + graph patterns
- **Full** (40KB, line 805): Core + agency system + advanced patterns

The Essential guide at lines 672-788 contains the foundational fanout guidance that ALL agents see:
- "Decision Framework: When to Decompose vs Implement" (lines 689-702)
- "Decomposition Pattern Templates" (lines 704-731)
- Pipeline, Fan-Out-Merge, Iterate-Until-Pass patterns

### 1.3 Key Fanout Instruction Sections

#### ETHOS_SECTION (`src/service/executor.rs:174-192`)
"The Graph is Alive" — encourages agents to grow the graph:
- "Task too large? → Fan out independent parts as parallel subtasks"
- "Prerequisite missing? → wg add..."
- "Follow-up needed? → wg add..."

#### Static AUTOPOIETIC_GUIDANCE (`src/service/executor.rs:197-259`)
Injected when `decomp_guidance = false`:
- "When to decompose vs implement directly"
- Good/bad reasons to decompose
- How to decompose (fan-out, pipeline, synthesis patterns)
- Guardrails: {{max_child_tasks}} and {{max_task_depth}} placeholders
- "When NOT to decompose"

#### Adaptive build_decomposition_guidance() (`src/service/executor.rs:419-505`)
Injected when `decomp_guidance = true` (default):
- Classifies task as ATOMIC or MULTI-STEP based on description signals
- ATOMIC tasks: "Implement directly without decomposition" + escape hatch
- MULTI-STEP tasks: Full decomposition templates + patterns
- Both get: validation criteria guidance + guardrails + "when NOT to decompose"

---

## 2. TB-Specific Prompt Injection

### 2.1 Condition G Meta-Prompts (TB-specific)

**File:** `terminal-bench/wg/adapter.py:515-563` — `CONDITION_G_META_PROMPT`

The ORIGINAL Condition G meta-prompt forces unconditional decomposition:
```
"You are a graph architect. You do NOT implement solutions yourself."
"DO NOT write code. DO NOT modify files. Only create wg tasks."
```

This is prepended to the task instruction when `autopoietic=True` in the condition config (line 790).

**File:** `terminal-bench/wg/adapter.py:583-638` — `CONDITION_G_SMART_META_PROMPT`

The SMART fanout replacement (used when `smart_fanout=True`):
- Try-first approach: "Strategy 1 — Direct Implementation (default)"
- Triage criteria: word count, file count, test count thresholds
- Mid-task switching: can switch to decomposition if context pressure detected
- Hard constraints: max 4 subtasks, max 1 level deep, no shared files

### 2.2 Prompt Injection Path in adapter.py

**File:** `terminal-bench/wg/adapter.py:784-797`

```python
if cfg.get("autopoietic"):
    if cfg.get("smart_fanout"):
        meta = CONDITION_G_SMART_META_PROMPT.replace("{seed_task_id}", task_id)
    else:
        meta = CONDITION_G_META_PROMPT.replace("{seed_task_id}", task_id)
    if verify_cmd:
        meta += f"\n## Test command\n..."
```

The meta-prompt is prepended to the task instruction, which then becomes the task description that the wg agent system sees. This means the meta-prompt lands in `vars.task_description` in `build_prompt()`, and the agent sees BOTH:
1. The TB meta-prompt (from adapter.py, in the description)
2. The wg decomposition guidance (from AUTOPOIETIC_GUIDANCE or build_decomposition_guidance, injected by build_prompt)

**This is a key tension**: The WG system's decomposition guidance says "default to implementing directly," but the TB Condition G meta-prompt says "DO NOT write code." They conflict.

### 2.3 Coordinator Heartbeat (TB-related)

**File:** `src/commands/service/coordinator_agent.rs:383-441`

The coordinator's autonomous heartbeat prompts are used for TB Condition G Phase 3. They inject time awareness:
```
[AUTONOMOUS HEARTBEAT] Tick #{tick_number} at {timestamp}
Time elapsed: {elapsed}s | Budget remaining: {remaining_display}
```

Phase guidance varies:
- Early: "Confirm task graph structure, verify agents are spawning..."
- Mid: "Assess agent progress..."
- Late: "WRAP-UP MODE — under 5 minutes remain..."

---

## 3. Guardrails Configuration and Enforcement

### 3.1 Config Knobs

**File:** `src/config.rs:430-478` — `GuardrailsConfig`

| Config Key | Default | Purpose |
|------------|---------|---------|
| `max_child_tasks_per_agent` | 10 | Max tasks a single agent can create via `wg add` |
| `max_task_depth` | 8 | Max depth of task dependency chains |
| `max_triage_attempts` | 3 | Max times a task can be requeued via failed-dep triage |
| `decomp_guidance` | true | Whether to use adaptive (true) or static (false) decomposition guidance |

### 3.2 Enforcement: Per-Agent Task Creation Limit

**File:** `src/commands/add.rs:275-293`

Enforced at `wg add` time when `WG_AGENT_ID` env var is set:
```rust
let count = count_agent_created_tasks(dir, agent_id);
if count >= max_child {
    anyhow::bail!("Agent {} has already created {}/{} tasks...");
}
```

Uses the provenance log to count how many `add_task` operations the agent has performed.

### 3.3 Enforcement: Task Depth Limit

**File:** `src/commands/add.rs:394-412`

Enforced at `wg add` time when `--after` is specified:
```rust
let max_parent_depth = effective_after.iter()
    .map(|parent_id| graph.task_depth(parent_id))
    .max().unwrap_or(0);
let new_depth = max_parent_depth + 1;
if new_depth > max_depth {
    anyhow::bail!("Task would be at depth {} (max: {})...");
}
```

**File:** `src/graph.rs:1456-1492` — `task_depth()`

Depth computed recursively with memoization and cycle detection. Depth = longest path from any root task.

### 3.4 Guardrail Values in Agent Prompts

**File:** `src/service/executor.rs:1032-1033`

TemplateVars gets populated from config:
```rust
max_child_tasks: guardrails.max_child_tasks_per_agent,
max_task_depth: guardrails.max_task_depth,
```

These appear in the agent prompt via either:
- `AUTOPOIETIC_GUIDANCE` template: `{{max_child_tasks}}` and `{{max_task_depth}}` placeholders (line 252-253)
- `build_decomposition_guidance()`: formatted directly into the string (lines 491-492)

---

## 4. Adaptive Decomposition Intelligence

### 4.1 Task Complexity Classification

**File:** `src/service/executor.rs:263-375` — `classify_task_complexity()`

Two-class scoring heuristic:

**ATOMIC signals** (lines 273-293): "single function", "fix bug in", "one file", "typo", "rename", "hotfix", "quick fix", etc.

**MULTI-STEP signals** (lines 296-328): "build pipeline", "multiple files", "end-to-end", "refactor", "implement feature", "pipeline", "first,", "second,", "then", etc.

**Scoring**: Count signal matches in description (case-insensitive). Also checks for 3+ bullet items as a multi-step signal. Ties go to multi-step. Default for no matches: >200 chars = multi-step, ≤200 chars = atomic.

### 4.2 Pattern Keywords Detection

**File:** `src/service/executor.rs:645-694`

Trigger keywords: "autopoietic", "self-organizing", "committee", "discussion", "deliberation", "swarm", "fork-join", "fan-out", "parallel", "loop", "cycle", "iterate", "research", "investigate", "audit"

When detected in the task description, the `PATTERN_KEYWORDS_GLOSSARY` is injected (line 792-794) explaining expected behavior for each pattern.

---

## 5. Existing Fanout Heuristics and Conditions

### 5.1 In the WG System (src/)

1. **Adaptive decomposition** (`decomp_guidance=true`): Classifies task as atomic/multi-step and tailors guidance
2. **Pattern keywords**: Detects organizational patterns in task descriptions and injects glossary
3. **Hard guardrails**: max_child_tasks_per_agent=10, max_task_depth=8 (enforced at `wg add`)
4. **Tiered knowledge**: Different guide detail levels based on model context window size
5. **Decision framework** (in essential guide): "Implement Directly If" / "Decompose If" criteria

### 5.2 In the TB System (terminal-bench/)

1. **CONDITION_G_META_PROMPT**: Forces unconditional decomposition ("DO NOT write code")
2. **CONDITION_G_SMART_META_PROMPT**: Try-first with triage criteria (word count, file count, test count)
3. **Architect bundle** (line 570-576): Restricts tool access to read-only + wg commands
4. **Heartbeat system**: Time-aware coordination prompts

### 5.3 Prior Research (Existing Analysis Documents)

- `terminal-bench/TB-CONDITION-G-FANOUT-ANALYSIS.md`: Full analysis showing 72% of tasks harmed by unconditional decomposition; break-even at ~45% context utilization + multi-phase structure
- `terminal-bench/DESIGN-smart-fanout-calculus.md`: Complete design for smart fanout with decision function, meta-prompt, and validation plan

---

## 6. Recommendations for Balanced Heuristic

### 6.1 Current State Summary

The WG system already has reasonable decomposition guidance:
- The adaptive classifier (`classify_task_complexity`) tries to discourage decomposition for atomic tasks
- The essential guide has a clear "Implement Directly If / Decompose If" framework
- Hard guardrails (10 tasks, depth 8) prevent runaway decomposition

But there are gaps:
- **No context-pressure signal**: Agents can't detect when they're approaching context limits
- **No mid-task switching**: The guidance is pre-task only; no support for "try then decompose"
- **No explicit file-scope constraint**: Nothing prevents parallel subtasks from modifying the same files
- **No subtask depth prohibition**: Workers can re-decompose (creating depth>1 chains)
- **TB tension**: The TB meta-prompt overrides the wg guidance for Condition G

### 6.2 What Agents Currently See vs What They Should See

**BEFORE (current agent prompt for a typical task):**
```
## Task Decomposition

This appears to be a **multi-step task**. Consider decomposing with dependencies.

### When to decompose
Decompose when the task has genuinely independent parts...
[full templates for pipeline, fan-out, iterate]

### Guardrails
- You can create up to **10** subtasks per session
- Task chains have a maximum depth of **8** levels
```

No mention of: trying direct implementation first, context pressure detection, file-scope constraints, or depth-1 prohibition for workers.

**AFTER (recommended additions):**
```
## Task Decomposition

This appears to be a **multi-step task**. Consider decomposing with dependencies.

### DEFAULT: Attempt direct implementation first
Even for multi-step tasks, start by implementing directly. Only switch to
decomposition if you observe:
- Context pressure (re-reading files you already read, losing track of changes)
- The task has 3+ truly independent sub-problems on different files
- Your turn count exceeds 25 with no test progress

### If you decompose
- Create at most 4 subtasks
- Each subtask must list its file scope — NO two subtasks may modify the same file
- Workers must NOT create their own subtasks (add "Implement directly" to descriptions)
- Always include a verify/integration task at the end
- Log your decision: wg log <task-id> "FANOUT_DECISION: decompose — <reason>"
```

### 6.3 File Paths That Need Modification

| File | Lines | Change Needed |
|------|-------|---------------|
| `src/service/executor.rs:197-259` | AUTOPOIETIC_GUIDANCE static | Add try-first language, file-scope constraint, depth-1 prohibition |
| `src/service/executor.rs:419-505` | build_decomposition_guidance() | Enhance MULTI-STEP guidance with try-first, context-pressure detection, file-scope |
| `src/service/executor.rs:273-328` | ATOMIC/MULTI_STEP signals | Consider adding more signals or adjusting thresholds |
| `src/commands/spawn/context.rs:689-702` | Essential guide decision framework | Add file-scope and depth-1 constraints |
| `src/config.rs:430-451` | GuardrailsConfig | Consider adding `max_subtask_depth` (default: 1) and `require_file_scope` (default: true) |
| `terminal-bench/wg/adapter.py:515-563` | CONDITION_G_META_PROMPT | Already has smart replacement; ensure smart_fanout is default for G |
| `terminal-bench/wg/adapter.py:583-638` | CONDITION_G_SMART_META_PROMPT | Already good; may need tuning after wg-side changes |

---

## Source References

| Source | Path | Key Content |
|--------|------|-------------|
| Prompt assembly | `src/service/executor.rs:762-885` | `build_prompt()` — main prompt construction |
| Static decomp guidance | `src/service/executor.rs:197-259` | `AUTOPOIETIC_GUIDANCE` constant |
| Adaptive decomp guidance | `src/service/executor.rs:419-505` | `build_decomposition_guidance()` function |
| Task complexity classifier | `src/service/executor.rs:263-375` | `classify_task_complexity()` with signal lists |
| Pattern keywords | `src/service/executor.rs:645-694` | Trigger keywords + glossary injection |
| Guardrails config | `src/config.rs:430-478` | `GuardrailsConfig` struct |
| Per-agent limit enforcement | `src/commands/add.rs:275-293` | `count_agent_created_tasks()` check |
| Depth limit enforcement | `src/commands/add.rs:394-412` | `task_depth()` check |
| Depth computation | `src/graph.rs:1456-1492` | `task_depth()` recursive with memoization |
| Essential guide | `src/commands/spawn/context.rs:667-788` | `build_essential_guide()` |
| Tiered guide | `src/commands/spawn/context.rs:644-815` | `build_tiered_guide()` |
| TemplateVars | `src/service/executor.rs:946-963` | Guardrail values plumbing |
| TB Condition G prompt | `terminal-bench/wg/adapter.py:515-563` | Original "always decompose" meta-prompt |
| TB Smart fanout prompt | `terminal-bench/wg/adapter.py:583-638` | Try-first smart meta-prompt |
| TB prompt injection | `terminal-bench/wg/adapter.py:784-797` | Where meta-prompt meets task description |
| Coordinator heartbeat | `src/commands/service/coordinator_agent.rs:383-441` | Time-aware heartbeat prompts |
| Prior fanout analysis | `terminal-bench/TB-CONDITION-G-FANOUT-ANALYSIS.md` | Full performance analysis |
| Smart fanout design | `terminal-bench/DESIGN-smart-fanout-calculus.md` | Decision function + validation plan |
| Ethos section | `src/service/executor.rs:174-192` | "The Graph is Alive" |
| Graph patterns section | `src/service/executor.rs:122-141` | Vocabulary + golden rules |
