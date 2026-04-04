# Condition B Trial Audit: Workgraph Usage Patterns

**Date:** 2026-04-03
**Auditor:** Architect agent (tb-audit-condition-b)
**Data source:** `terminal-bench/results/full-condition-b/full-condition-b/`
**Model:** Minimax M2.7 via OpenRouter

---

## Executive Summary

Workgraph (wg) tools **are being used** in Condition B trials, but usage is **inconsistent and largely superficial**. Only 45% of completed trials invoke any wg tool at all, and only 5% use structured decomposition (creating subtasks and completing them). The wg tools function primarily as **journaling/bookkeeping scaffolding** rather than enabling stigmergic multi-agent coordination. The current Condition B is testing **single-agent task management overhead**, not stigmergic coordination.

---

## Dataset Overview

| Metric | Value |
|--------|-------|
| Total trial dirs | 267 |
| Docker/infra errors | 168 (63%) |
| Completed with transcripts | 98 |
| Overall pass rate (completed) | 43/98 (43.9%) |

The high error rate is due to Docker Hub rate limiting (`toomanyrequests`), not agent failures.

---

## WG Usage Quantitative Summary

### Usage Frequency

| Category | Trials | % of completed | Pass rate | Avg turns |
|----------|--------|----------------|-----------|-----------|
| **No wg usage** | 54 | 55.1% | 42.6% | 26.7 |
| **Any wg usage** | 44 | 44.9% | 45.5% | 31.0 |
| Total | 98 | 100% | 43.9% | 28.6 |

### Usage Pattern Classification

| Pattern | Trials | Pass rate | Avg turns | Description |
|---------|--------|-----------|-----------|-------------|
| `none` | 54 | 42.6% | 26.7 | Agent never calls any wg tool |
| `bookkeeping` | 15 | 60.0% | 24.9 | Only wg_log + wg_done (no decomposition) |
| `log-only` | 11 | 9.1% | 49.5 | Only wg_log at start, never completes via wg_done |
| `attempted` | 10 | 40.0% | 34.3 | Creates subtasks but doesn't complete them |
| `structured` | 5 | 80.0% | 15.2 | Creates subtasks AND completes them |
| `minimal` | 3 | 66.7% | 8.3 | Only wg_done (no logging or decomposition) |

### WG Tool Call Distribution (across all 165 wg calls)

| Tool | Count | % | Purpose |
|------|-------|---|---------|
| `wg_log` | 89 | 54% | Progress journaling |
| `wg_done` | 43 | 26% | Task completion |
| `wg_add` | 32 | 19% | Subtask creation |
| `wg_list` | 1 | 1% | Status checking |
| `wg_show` | 0 | 0% | Never used |
| `wg_fail` | 0 | 0% | Never used |
| `wg_artifact` | 0 | 0% | Never used |
| `wg_msg_send` | 0 | 0% | Never used |
| `wg_msg_read` | 0 | 0% | Never used |

### WG Overhead

- **WG turns / total turns** (wg-using trials): 137 / 1363 = **10.1%**
- **Avg wg calls per wg-using trial:** 3.8
- **Avg subtasks created** (when decomposing): 2.1

---

## Qualitative Findings

### 1. The model understands what wg is for

When agents use wg tools, they use them correctly:
- `wg_log` messages are contextually appropriate (e.g., "Starting Nginx web server setup...")
- `wg_add` titles and descriptions are meaningful decompositions
- `wg_done` is called with correct task IDs
- The system prompt from `build_condition_b_prompt()` gives sufficient context

### 2. Usage is NOT stigmergic — it's single-agent task management

All wg interactions are **within the same agent session**. There is:
- No multi-agent coordination (agents create and complete their own subtasks)
- No message passing between agents (`wg_msg_send`/`wg_msg_read` = 0 calls)
- No artifact sharing (`wg_artifact` = 0 calls)
- No status checking of other agents' work (`wg_show` = 0 calls)
- The wg graph lives in an isolated temp directory per trial — there's no shared graph

### 3. Structured decomposition correlates with success

The 5 trials with structured decomposition (wg_add + wg_done pattern) had:
- **80% pass rate** (vs 42.6% for no-wg trials)
- **15.2 avg turns** (vs 26.7 for no-wg trials)
- Example: `nginx-request-logging` decomposed into 5 subtasks, completed all in 16 turns

These trials show the agent using wg as a **personal checklist** — decomposing the task, working through subtasks sequentially, marking each done. This is effective but is self-organization, not coordination.

### 4. Log-only usage is a negative signal

Trials that only log once at the start but never call wg_done have a **9.1% pass rate** and use **49.5 avg turns** (hitting the 50-turn limit). This suggests the agent starts with good intentions but gets bogged down in the actual task, never reaching a completion state.

### 5. Non-wg trials don't even mention wg

Of 54 trials with no wg tool calls, **zero** mention "workgraph" or "wg_" in assistant text. The model either engages with the wg tools immediately or ignores them entirely. There's no "tried and couldn't" behavior.

### 6. First-turn behavior is split

| First tool used | Count |
|-----------------|-------|
| wg tool (log/add) | 29 (30%) |
| bash | 38 (39%) |
| Other (read/write/etc) | 31 (32%) |

The model's decision to engage with wg appears task-dependent and possibly stochastic (temperature=0.0 notwithstanding, different task prompts elicit different behavior).

---

## Structured Decomposition Examples

### Success: `kv-store-grpc__xwccgyG` (reward=1.0, 17 turns)
```
Turn 0: wg_add "Install grpcio packages"
Turn 0: wg_add "Create proto file"
Turn 0: wg_add "Generate Python gRPC code"
Turn 0: wg_add "Create and run server.py"
Turn 1: wg_log → work begins
Turn 2: wg_done "install-grpcio-packages"
Turn 4: wg_done "create-proto-file"
Turn 5: wg_list → checks status
Turn 14: wg_done (4 tasks completed)
Turn 15: wg_log → summary
```
Pattern: Decompose → execute sequentially → mark each done → verify

### Success: `pypi-server__MWofR3Z` (reward=1.0, 22 turns)
```
Turn 0: wg_add "Create vectorops package structure"
Turn 0: wg_add "Build vectorops package"
Turn 0: wg_add "Set up PyPI server and host package"
Turn 0: wg_add "Test package installation"
Turn 1-20: Sequential execution with log + done after each
```
Pattern: 4-step pipeline, agent works through each sequentially

---

## Within-Task Comparisons

For tasks where both wg-using and non-using attempts exist:

| Task | wg result | no-wg result |
|------|-----------|--------------|
| break-filter-js-from-html | 0.0 | 1.0 |
| distribution-search | 1.0 | 1.0 |
| largest-eigenval | 1.0 | 0.0, 0.0 |
| make-mips-interpreter | 0.0 | 0.0 |
| overfull-hbox | 1.0 | 1.0 |
| password-recovery | 0.0 | 1.0, 0.0 |
| portfolio-optimization | 0.0 | 1.0, 1.0 |
| winning-avg-corewars | 0.0 | 0.0, 0.0 |

No clear advantage for wg usage in within-task comparisons (n too small for significance).

---

## Architectural Analysis

### Why no stigmergic coordination is happening

The adapter (`wg/adapter.py`) creates an **isolated workgraph per trial**:
```python
self._wg_temp_dir = tempfile.mkdtemp(prefix="tb-wg-")
```

- Each trial gets its own temp directory
- wg commands run on the host, not in the container
- There's no mechanism for multiple agents to share a graph
- The root task is created automatically but there's no coordinator dispatching subtasks
- When the agent creates subtasks via `wg_add`, nobody else picks them up — the same agent does all the work sequentially

### What Condition B actually tests

Condition B tests: **Does adding task management tools (decomposition, journaling, completion tracking) to a single agent improve performance?**

It does NOT test: **Does multi-agent stigmergic coordination via a shared task graph improve performance?**

---

## Recommendations

### Is current Condition B testing stigmergic coordination?

**No.** It's testing tool pollution (55% of agents ignore the tools) mixed with single-agent task management (45% use tools, mostly superficially). The 5% that use structured decomposition show promise but are still single-agent.

### For a true stigmergic Condition C, the experiment would need:

1. **Shared graph**: Multiple agents reading/writing the same workgraph
2. **Coordinator dispatch**: A coordinator that assigns subtasks to different agents
3. **Agent handoffs**: One agent creates subtasks, other agents claim and execute them
4. **Communication**: `wg_msg_send`/`wg_msg_read` to coordinate between agents
5. **Longer time horizons**: Multi-agent coordination benefits compound over time

### For improving Condition B as-is:

1. **Strengthen the system prompt**: The current prompt is minimal. Consider injecting the wg skill/quickstart to increase wg adoption beyond 45%
2. **Add a decomposition heuristic**: For tasks over a certain complexity, encourage the agent to decompose first
3. **Track wg usage as a metric**: Record whether wg was used and what pattern, alongside pass/fail
4. **Remove unused tools**: `wg_msg_send`, `wg_msg_read`, `wg_artifact`, `wg_show` are never used — they add noise to the tool list
