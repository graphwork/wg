# Audit: Terminal Bench WG Adapter Bypass Points

**Task:** audit-current-tb
**Date:** 2026-04-05
**File audited:** `terminal-bench/wg/adapter.py` (1730 lines)
**Supporting files:** `terminal-bench/wg/tb_logging.py`, `terminal-bench/wg/__init__.py`, `terminal-bench/tb_trial_runner.py`, `terminal-bench/tb_collect_results.py`

---

## 1. WG Bypass Points — Complete Catalog

### 1.1 Agent Loop (adapter.py:1351–1541)

The adapter reimplements the entire agent execution loop that native wg provides via its executor system (`src/commands/service/coordinator.rs` → executor dispatch).

| Bypass Point | File:Line | What adapter does | What native wg does |
|---|---|---|---|
| **Agent loop** | adapter.py:1450–1541 | Custom `for turn in range(self.max_turns)` loop with litellm | Coordinator spawns executor (claude/amplifier/shell) per task |
| **Tool dispatch** | adapter.py:817–875 | `execute_tool()` switch on tool name strings | Native executor has built-in tool handling via Claude Code or amplifier |
| **Context management** | adapter.py:1427–1429 | Manual `messages` list accumulation | Native uses `--context-scope` (clean/task/graph/full) + compaction |
| **System prompt** | adapter.py:882–1229 | Six hand-crafted `build_condition_*_prompt()` functions | Native uses `src/agency/prompt.rs` prompt composition from role/tradeoff components |
| **Message truncation** | adapter.py:1511–1513 | Truncate tool output >50K chars | Native executor handles context pressure via streaming + compaction |
| **Turn limit** | adapter.py:1450 | `max_turns` parameter (100–1M depending on condition) | Native has no turn limit; uses timeout (`--verify-timeout`) |
| **Termination detection** | adapter.py:1494–1497, 1536–1541 | Checks for `wg_done`/`wg_fail` on root_task_id | Native coordinator monitors task status transitions |
| **Token tracking** | adapter.py:1581–1584 | Manual accumulation from litellm response | Native tracks via executor response metadata |
| **Error handling** | adapter.py:1462–1465 | Catch-and-break on LLM call failure | Native executor has retry logic, rate-limit backoff |

### 1.2 Graph State Management (adapter.py:1296–1349)

| Bypass Point | File:Line | What adapter does | What native wg does |
|---|---|---|---|
| **Graph init** | adapter.py:1300–1311 | `tempfile.mkdtemp()` + `wg init` on host | Graph lives in project `.wg/` directory |
| **Graph lifecycle** | adapter.py:1592–1603 | `shutil.copytree()` to logs, then `shutil.rmtree()` | Persistent graph; no cleanup |
| **Root task creation** | adapter.py:1365–1370 (F), 1375–1380 (E), etc. | `wg add` with `uuid.uuid4().hex[:8]` ID | Tasks created by user/coordinator with meaningful IDs |
| **Agency bootstrap** | adapter.py:1313–1327 (D), 1330–1345 (E) | `wg agency init` + hardcoded agent create | Native uses `wg agency init` interactively; agents assigned via `.assign-*` pipeline |
| **Agent assignment** | adapter.py:1382, 1397 | `wg assign` with hardcoded names | Native uses LLM-based placement (`.place-*` → `.assign-*`) |

### 1.3 Tool Schema Reimplementation (adapter.py:44–503)

Every tool is defined as an OpenAI function-calling JSON schema, bypassing native wg's tool system entirely.

| Tool Category | Tools | Lines | Notes |
|---|---|---|---|
| **File/bash tools** | `bash`, `read_file`, `write_file`, `edit_file`, `glob`, `grep` | 44–209 | Reimplements Claude Code's built-in tools as custom JSON schemas |
| **Web tools** | `web_search`, `web_fetch` | 211–257 | Not present in native wg — adapter-specific additions |
| **WG tools** | `wg_show`, `wg_list`, `wg_add`, `wg_done`, `wg_fail`, `wg_log`, `wg_artifact`, `wg_msg_send`, `wg_msg_read` | 272–423 | Reimplements wg CLI commands as function-call tools |
| **Condition F enhanced** | `wg_add` (with `verify` + `id` params) | 447–503 | Extended version of wg_add; native CLI already supports these flags |

### 1.4 Condition-Specific Tool Sets (adapter.py:259–503)

| Condition | Tool Set | Lines | Difference from native |
|---|---|---|---|
| A (control) | `CONDITION_A_TOOLS` (8 tools) | 260–269 | No wg tools at all — bare agent baseline |
| B (treatment) | `CONDITION_B_TOOLS` (17 tools) | 425–435 | Full wg tool access via adapter proxy |
| C (treatment) | `CONDITION_C_TOOLS` = B | 438 | Same tools, different prompt (skill injection) |
| D (treatment) | `CONDITION_D_TOOLS` = B | 441 | Same tools, different prompt + agency setup |
| E (treatment) | `CONDITION_E_TOOLS` = B | 444 | Same tools, different prompt + orchestrator identity |
| F (treatment) | `CONDITION_F_TOOLS` (17 tools) | 485–503 | Enhanced `wg_add` with `verify`/`id` params |

---

## 2. WG Tool → Native WG Command Mapping

Every `wg_*` tool routes through `_exec_wg_cmd_host()` (adapter.py:788–814), which runs the real `wg` binary on the host against the temp directory.

| Adapter Tool | `execute_tool()` Handler | Native wg CLI Equivalent | Differences |
|---|---|---|---|
| `wg_show(task_id)` | adapter.py:841–842 | `wg show <task_id>` | Identical (pass-through) |
| `wg_list(status?)` | adapter.py:843–847 | `wg list [--status X]` | Identical |
| `wg_add(title, after?, desc?, verify?, id?)` | adapter.py:848–858 | `wg add "title" [--after X] [-d "desc"] [--verify "cmd"] [--id X]` | F-condition adds `--verify`/`--id`; B-E lack these |
| `wg_done(task_id, converged?)` | adapter.py:859–863 | `wg done <task_id> [--converged]` | Identical |
| `wg_fail(task_id, reason)` | adapter.py:864–865 | `wg fail <task_id> --reason "reason"` | Identical |
| `wg_log(task_id, message)` | adapter.py:866–867 | `wg log <task_id> "message"` | Identical |
| `wg_artifact(task_id, path)` | adapter.py:868–869 | `wg artifact <task_id> path` | Identical |
| `wg_msg_send(task_id, message)` | adapter.py:870–871 | `wg msg send <task_id> "message"` | Identical |
| `wg_msg_read(task_id)` | adapter.py:872–873 | `wg msg read <task_id>` | Identical |

**Key finding:** The wg tools themselves are thin pass-throughs to the real binary. The bypass is not in the tools — it's in the agent loop, prompt construction, and graph lifecycle management that wrap them.

---

## 3. LiteLLM Loop Configuration

### Model (adapter.py:37, 1434–1435)
- **Constant:** `BENCHMARK_MODEL = "openrouter:minimax/minimax-m2.7"`
- **Format conversion:** `model_raw.replace(":", "/", 1)` converts wg-style `provider:model` to litellm `provider/model`
- **Override:** `self.model_name or BENCHMARK_MODEL` — CLI can override via `--model_name`

### LLM Call (adapter.py:1453–1461)
```python
response = await litellm.acompletion(
    model=model,                    # "openrouter/minimax/minimax-m2.7"
    messages=messages,              # system + user + assistant + tool history
    tools=tools,                    # condition-specific tool set (8–17 tools)
    tool_choice="auto",             # LLM decides when to use tools
    temperature=self.temperature,   # default 0.0
    max_tokens=16384,               # hardcoded output token limit
)
```

### Prompting
Six condition-specific system prompts (adapter.py:882–1229):

| Condition | Builder | Key prompt features |
|---|---|---|
| A | `build_condition_a_prompt()` :882 | Bare: "You are a coding agent", guidelines only |
| B | `build_condition_b_prompt()` :901 | + root_task_id, wg patterns, journal/resume |
| C | `build_condition_c_prompt()` :927 | + "external memory" framing, mandatory planning phase, decomposition guidance |
| D | `build_condition_d_prompt()` :968 | + agency identity (name/role/tradeoff), Attempt→Verify→Iterate→Declare loop, 3-iteration fail cap |
| E | `build_condition_e_prompt()` :1016 | + orchestrator identity, Organize→Implement→Verify→Triage protocol, 6-iteration fail cap, independent verification |
| F | `build_condition_f_prompt()` :1097 | + distilled wg context injection, test-first discovery, adaptive decomposition, 5-iteration cap, 30-min time budget |

### Message History (adapter.py:1427–1429, 1474, 1518–1522)
- Initial: `[system_prompt, user_instruction]`
- Each turn: append assistant message + tool results
- No context window management — messages grow unboundedly (truncation only on individual tool outputs >50K chars)

### Turn Limits per Condition
| Condition | max_turns | Set at |
|---|---|---|
| A, B, C | 100 | adapter.py:1268 (default) |
| D | 200 | adapter.py:1691 |
| E | 300 | adapter.py:1705 |
| F | 1,000,000 | adapter.py:1728 (effectively unlimited) |

---

## 4. Harbor Integration Points

### BaseAgent Implementation (adapter.py:1236–1258)
- `WorkgraphAgent` extends `harbor.agents.base.BaseAgent`
- Required interface: `name()`, `version()`, `setup(environment)`, `run(instruction, environment, context)`

### Environment (Harbor's `BaseEnvironment`)
- `env.exec(command, timeout_sec)` — used for ALL file/bash tool execution inside the Docker container
- Returns `ExecResult` with `.stdout`, `.stderr`, `.return_code`
- Used in: `_exec_bash()` :510, `_exec_read_file()` :527, `_exec_write_file()` :546, `_exec_edit_file()` :563, `_exec_glob()` :590, `_exec_grep()` :623

### Tools that run on HOST (not in container)
| Tool | Why host-side |
|---|---|
| `_exec_wg_cmd_host()` :788 | wg binary not in Docker container; graph state on host |
| `_exec_web_search()` :666 | Uses `ddgs`/`duckduckgo_search` Python library on host |
| `_exec_web_fetch()` :702 | Uses `httpx`/`trafilatura` on host |

### AgentContext (adapter.py:1581–1584)
- `context.n_input_tokens` — total input tokens
- `context.n_output_tokens` — total output tokens
- `context.cost_usd` — total cost
- `context.metadata` — condition-specific metadata dict

### setup() (adapter.py:1296–1349)
- For conditions B–F: creates temp dir, runs `wg init`
- For D: bootstraps agency, creates "solver" agent (programmer/careful)
- For E: bootstraps agency, creates "orchestrator" agent (architect/thorough)
- For F: no agency bootstrap

### Convenience Agent Classes (adapter.py:1642–1729)
Six Harbor-importable classes, each setting condition + BENCHMARK_MODEL:
- `ConditionAAgent` → `wg.adapter:ConditionAAgent`
- `ConditionBAgent` → `wg.adapter:ConditionBAgent`
- `ConditionCAgent` → `wg.adapter:ConditionCAgent`
- `ConditionDAgent` → `wg.adapter:ConditionDAgent`
- `ConditionEAgent` → `wg.adapter:ConditionEAgent`
- `ConditionFAgent` → `wg.adapter:ConditionFAgent`

Harbor invocation: `harbor run --agent-import-path wg.adapter:ConditionBAgent`

---

## 5. Graph State Per Trial

### Creation (adapter.py:1299–1311)
1. `tempfile.mkdtemp(prefix="tb-wg-")` → e.g. `/tmp/tb-wg-abc123/`
2. `wg init` inside that temp dir → `/tmp/tb-wg-abc123/.wg/`
3. (D/E only) `wg agency init` + `wg agent create`
4. Root task created via `wg add` with UUID-based ID

### During trial
- All `wg_*` tools operate on `self._wg_graph_dir` via `_exec_wg_cmd_host()`
- Agent can create subtasks, log, mark done/fail — all against the temp graph
- WG snapshots (`wg list`) captured at: after_init, after_decomposition (D/E/F), before_done (D/E/F)

### Destruction (adapter.py:1592–1603)
1. `shutil.copytree(wg_dir, logs_dir / "workgraph_state")` — preserve for analysis
2. `shutil.rmtree(self._wg_temp_dir)` — clean up

---

## 6. Other TB Files Referencing WG

### `terminal-bench/tb_trial_runner.py`
- **Purpose:** Creates wg tasks from TB task definitions using the real `wg` CLI on the host
- **WG usage:** Direct `subprocess.run(["wg", ...])` calls (line 165–175)
- **Conditions A, C, D, E** defined with context_scope/tags (lines 128–149)
- **Creates fan-out tasks** (one per condition×task×replica) + fan-in results collection
- **NOT using the adapter** — this is a separate path that creates tasks in the project's real wg

### `terminal-bench/tb_collect_results.py`
- **Purpose:** Fan-in analysis — reads FLIP scores, eval scores, verify results
- **WG usage:** `subprocess.run(["wg", "show", task_id])` to get task status
- **Reads:** `.wg/agency/evaluations/` for FLIP and LLM evaluation data

### `terminal-bench/wg/tb_logging.py`
- **Purpose:** Structured NDJSON logging for all conditions
- **WG awareness:** Tracks `wg_*` tool calls, verification commands, decomposition tasks, triage count
- **No direct wg interaction** — purely observational logging

### `terminal-bench/wg/__init__.py`
- Exports: `WorkgraphAgent`, `ConditionAAgent`, `ConditionBAgent`, `ConditionCAgent`, `ConditionDAgent`, `ConditionEAgent`
- **Missing:** `ConditionFAgent` not exported

---

## 7. What Would Break If We Swapped to Native Executor

### Would work out of the box
- **wg_* tools** — already pass-through to real binary; native executor has equivalent via CLI
- **Graph state** — native executor operates on the project's real graph, not a temp dir
- **Agency** — native handles `.place-*` → `.assign-*` pipeline automatically

### Would need replacement
| Adapter Feature | Native Equivalent | Migration Effort |
|---|---|---|
| **litellm agent loop** (adapter.py:1450–1541) | Native executor (claude/amplifier/shell) | **Large** — entire loop replaced by executor config |
| **Custom tool schemas** (adapter.py:44–503) | Claude Code built-in tools + wg CLI access | **Medium** — remove all schema defs; native executor provides tools |
| **`env.exec()` for bash/file** (adapter.py:510–663) | Claude Code's Bash/Read/Write/Edit/Glob/Grep | **Medium** — Harbor container execution replaced by native sandbox |
| **Host-side web tools** (adapter.py:666–785) | Claude Code's WebSearch/WebFetch | **Small** — already available in Claude Code |
| **Condition-specific prompts** (adapter.py:882–1229) | Agency prompt composition (`src/agency/prompt.rs`) | **Large** — six prompts need conversion to role/tradeoff components |
| **TrialLogger** (tb_logging.py) | No native equivalent for benchmark logging | **Must keep** — or build wg-native telemetry hook |
| **Temp graph lifecycle** (adapter.py:1299–1603) | Tasks in real project graph | **Design change** — trial isolation strategy needed |
| **Harbor BaseAgent interface** | No equivalent — Harbor-specific | **Must keep or adapt** — Harbor expects `setup()`/`run()` interface |
| **Turn limit / termination** | Native timeout-based | **Small** — use `--verify-timeout` instead |
| **Token tracking** | Native tracks via executor | **Small** — read from executor metadata |

### Would be lost (adapter-only features)
- **Condition A** (no-wg baseline) — native executor always has wg; need a "disable wg tools" mode
- **Per-trial graph isolation** — native uses shared project graph; need worktree-per-trial or namespace isolation
- **BENCHMARK_MODEL enforcement** — native uses per-task/executor/coordinator model selection; need a way to pin model for reproducibility
- **Harbor container execution** — native runs in host or worktree, not Docker; changes the security/isolation model

### Key architectural tension
The adapter exists because Harbor provides **containerized execution** (Docker environments with `env.exec()`) while native wg provides **host/worktree execution**. Swapping to native executor means either:
1. Running native wg inside Harbor containers (requires wg binary injection, which the adapter explicitly avoids — see adapter.py:788 comment)
2. Running Harbor tasks outside containers (loses isolation)
3. A hybrid: native wg executor that delegates file/bash to Harbor's `env.exec()` (new executor type)

