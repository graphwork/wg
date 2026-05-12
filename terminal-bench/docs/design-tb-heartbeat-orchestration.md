# Design: TB Heartbeat-Orchestrated Coordinator (Condition G, Phase 3)

**Date:** 2026-04-08
**Task:** design-tb-heartbeat-orchestration
**Status:** Design complete
**Depends on:**
- [research-condition-g-status.md](research-condition-g-status.md) — Condition G current state and history
- [research-coordinator-heartbeat.md](research-coordinator-heartbeat.md) — Heartbeat pattern options and architecture
- [research-agent-web-prompting.md](research-agent-web-prompting.md) — Web tool gaps and fan-out patterns

---

## 1. Executive Summary

Condition G has evolved through two phases: Phase 1 (F-without-surveillance, context-only) and Phase 2 (autopoietic, agent-builds-own-graph). Phase 2 reached 64% pass rate on TB 2.0 but suffers from prompt competition, convergence signaling failures, and model capability limitations with M2.7.

This design defines **Phase 3: heartbeat-orchestrated coordinator**. Instead of asking the seed agent to build its own wg (Phase 2), the coordinator itself runs as a persistent strategist on a 30-second heartbeat loop — reviewing graph state, dispatching work, recovering from failures, and adapting strategy. This replicates the quality of interactive coordinator sessions in autonomous mode.

### Key Changes from Phase 2

| Aspect | Phase 2 (Autopoietic) | Phase 3 (Heartbeat) |
|--------|----------------------|---------------------|
| Who orchestrates | Seed agent (via meta-prompt) | Coordinator agent (via heartbeat loop) |
| Dispatch strategy | Fire-and-forget (agents spawned on ready) | Coordinator reviews + dispatches every 30s |
| Failure recovery | None (mechanical cycle restart) | Coordinator reasons about failures, re-plans |
| Context awareness | Agent sees own task only | Coordinator sees full graph + events + agents |
| Prompt competition | REQUIRED_WORKFLOW vs meta-prompt | No conflict (coordinator prompt is separate) |
| Web research | Not available | Available via prompt hints (P0), native tools (P1) |

---

## 2. Architecture

### 2.1 System Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                    wg service daemon                         │
│                                                             │
│  ┌─────────────────────┐    ┌──────────────────────────┐   │
│  │   Event Loop         │    │  Coordinator Agent (LLM) │   │
│  │                      │    │                          │   │
│  │  poll(epoll, 30s)    │    │  - Persistent session    │   │
│  │  ├─ IPC: GraphChanged│───▶│  - Per-turn context:     │   │
│  │  ├─ IPC: UserChat    │    │    • graph summary       │   │
│  │  ├─ Heartbeat timer  │───▶│    • recent events       │   │
│  │  │   (every 30s)     │    │    • active agents       │   │
│  │  └─ Safety net (60s) │    │    • failed tasks        │   │
│  │                      │    │    • attention items      │   │
│  │  Tick phases:        │    │                          │   │
│  │  1. Cleanup/reap     │    │  Actions:                │   │
│  │  2. Cycle maint.     │    │  - wg add (create tasks) │   │
│  │  3. Auto-assign      │    │  - wg kill (stuck agent) │   │
│  │  4. Auto-evaluate    │    │  - wg msg (guide agent)  │   │
│  │  5. Check ready      │    │  - wg fail (abandon bad  │   │
│  │  6. Spawn agents ◀───│────│    approach)             │   │
│  └─────────────────────┘    └──────────────────────────┘   │
│            │                                                 │
│            ▼                                                 │
│  ┌─────────────────────────────────────────────────────┐   │
│  │              Agent Pool (max_agents=8)                │   │
│  │                                                       │   │
│  │  ┌──────────┐ ┌──────────┐ ┌──────────┐             │   │
│  │  │ Agent 1  │ │ Agent 2  │ │ Agent 3  │  ...        │   │
│  │  │ (impl)   │ │ (impl)   │ │ (verify) │             │   │
│  │  │ worktree │ │ worktree │ │ worktree │             │   │
│  │  └──────────┘ └──────────┘ └──────────┘             │   │
│  └─────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 Heartbeat Flow (Per Tick)

```
                    30s timer fires
                         │
                         ▼
              ┌─────────────────────┐
              │ Build context snapshot│
              │  (graph + events +   │
              │   agents + failures) │
              └──────────┬──────────┘
                         │
                         ▼
              ┌─────────────────────┐
              │ Inject heartbeat     │
              │ prompt into          │
              │ coordinator agent    │
              └──────────┬──────────┘
                         │
                         ▼
              ┌─────────────────────┐
              │ Coordinator reasons: │
              │ • All healthy? → NOOP│
              │ • Stuck agent? → kill│
              │ • Failed task? → plan│
              │ • Ready work? → add  │
              └──────────┬──────────┘
                         │
                         ▼
              ┌─────────────────────┐
              │ Execute actions      │
              │ (wg add/kill/msg/    │
              │  fail via bash)      │
              └──────────┬──────────┘
                         │
                         ▼
              ┌─────────────────────┐
              │ Tick phases 1-6      │
              │ (cleanup, spawn,     │
              │  cycle maintenance)  │
              └─────────────────────┘
```

### 2.3 TB Trial Lifecycle

```
TB runner (adapter.py)
  │
  ├─ 1. Create seed task with TB instruction + verify command
  ├─ 2. Configure coordinator: heartbeat_interval=30, max_agents=8
  ├─ 3. Start service: wg service start
  │       │
  │       ├─ Coordinator agent spawns
  │       ├─ Seed task is ready → agent spawns
  │       ├─ Every 30s: heartbeat → coordinator reviews
  │       ├─ Agent completes → coordinator sees, dispatches next
  │       ├─ Agent fails → coordinator re-plans
  │       └─ All done or timeout → service stops
  │
  └─ 4. Run verify command → reward.txt
```

---

## 3. Coordinator Configuration

### 3.1 Config for TB Heartbeat Runs

```toml
# .wg/config.toml
[coordinator]
coordinator_agent = true
heartbeat_interval = 30       # seconds between synthetic heartbeats
poll_interval = 60            # background safety net (unchanged)
settling_delay_ms = 2000      # debounce for GraphChanged (unchanged)
max_agents = 8                # parallel agent limit
model = "sonnet"              # coordinator model (strategic reasoning)

[coordinator.context]
scope = "graph"               # full graph context per heartbeat turn
```

### 3.2 Condition G Phase 3 Config (adapter.py)

```python
CONDITION_CONFIG["G"] = {
    "context_scope": "graph",
    "exec_mode": "full",
    "exclude_wg_tools": False,
    "max_agents": 8,
    "autopoietic": False,            # CHANGED: no autopoietic meta-prompt
    "coordinator_agent": True,
    "heartbeat_interval": 30,         # NEW: 30s heartbeat
    "coordinator_model": "sonnet",    # NEW: use capable model for coordinator
}
```

### 3.3 Key Design Decisions

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| `heartbeat_interval` | 30s | Matches the existing `coordinator.interval` default; fast enough to catch stuck agents within 1 minute |
| `max_agents` | 8 | Unchanged from Phase 2; allows meaningful parallelism |
| `autopoietic` | false | Phase 3 removes the autopoietic meta-prompt — the coordinator orchestrates, not the seed agent |
| `coordinator_model` | sonnet | Coordinator needs strategic reasoning; M2.7 struggles with meta-cognition (Phase 2 finding). Task agents still use the benchmark model |
| `context_scope` | graph | Agents see graph context for cross-task awareness |

### 3.4 Heartbeat Prompt Template

```
[AUTONOMOUS HEARTBEAT] Tick #{{tick_number}} at {{timestamp}}
Time elapsed: {{elapsed}}s | Budget remaining: ~{{remaining}}s

You are the autonomous coordinator for a Terminal-Bench trial. No human operator.
Review the system state and take action:

1. STUCK AGENTS: Any agent running >5min with no output? → `wg kill <id>` and retry
2. FAILED TASKS: Any tasks failed? → Analyze cause, create fix-up task or retry
3. READY WORK: Unblocked tasks waiting? → Ensure they'll be dispatched (they auto-spawn)
4. PROGRESS CHECK: Is the trial converging toward passing tests?
5. STRATEGIC: Should any running approach be abandoned for a different strategy?

If everything is nominal, respond: "NOOP — all systems nominal."
If you take action, log what and why.
```

---

## 4. Agent Prompting Changes

### 4.1 Current Gaps (from research-agent-web-prompting.md)

1. **No web tools in native executor** — agents cannot search the web
2. **No prompt mentions web capabilities** — agents don't know they could research
3. **No research-first pattern** — Required Workflow goes straight to implement
4. **Fan-out is implementation-only** — no research intent in Condition G

### 4.2 Prompt Additions

#### 4.2.1 Research Hint Section (P0 — No Code Changes)

Add to task descriptions for tasks requiring specialized knowledge:

```
## Research Hints

If you encounter unfamiliar APIs, languages, or libraries:
- Check for existing documentation in the workspace
- Read error messages carefully — they often contain the fix
- For build systems (CMake, Cython, Cargo): check for existing config files first
```

This works without web tools by directing agents to use what's available.

#### 4.2.2 Available Capabilities Section (P1 — After Web Tools Added)

Add to `build_prompt()` when web tools are in the tool registry:

```
## Available Capabilities

You have access to web tools for research:
- `web_search "query"` — Search for documentation, examples, and solutions
- `web_fetch "url"` — Read a web page for documentation or reference

When facing unfamiliar technologies, search for documentation before implementing.
```

Guard: Only inject when `tool_registry.has("web_search")` is true.

#### 4.2.3 Research Phase in Required Workflow (P2)

Add as Step 0 in the `REQUIRED_WORKFLOW_SECTION`:

```
0. **Understand the problem** before implementing:
   - Read all referenced files and test cases
   - If the task involves unfamiliar technology, search for documentation
   - Check existing patterns in the codebase (grep for similar implementations)
```

### 4.3 Coordinator Prompt Addition

Add to the coordinator agent's system prompt:

```
## Task Dispatch Strategy

When creating tasks for complex or unfamiliar problems:
- Add a research subtask first (--exec-mode light) that investigates approaches
- Use fan-out for independent work (research || design || implement)
- Always include a verify task at the end of any chain
- For TB tasks: the verify command in the seed task is the ultimate acceptance test
```

---

## 5. Multi-Agent Fan-Out Patterns

### 5.1 Pattern A: Simple Pipeline (Most TB Tasks)

For straightforward implementation tasks where the approach is clear:

```
Seed Task (TB instruction)
    │
    ▼
  Agent 1: Implement
    │
    ▼
  Agent 2: Verify (run tests, fix regressions)
    │
    ▼
  Done
```

**When to use:** Task is well-specified, no unfamiliar tech, clear path to solution.
**Files touched:** All agents work on the same files sequentially (pipeline, not parallel).

### 5.2 Pattern B: Research → Implement → Verify

For tasks requiring domain knowledge the agent may not have:

```
Seed Task
    │
    ├─▶ Agent 1: Research (exec_mode=light)
    │     "Find how to configure X, document in /tmp/research.md"
    │
    ▼
  Agent 2: Implement (depends on research)
    │     "Using research findings, implement X"
    │
    ▼
  Agent 3: Verify
    │
    ▼
  Done
```

**When to use:** COBOL, Cython, constraint programming, unfamiliar build systems.
**Key constraint:** Research agent produces a document, implement agent reads it. No parallel file modification.

### 5.3 Pattern C: Parallel Implementation (Independent Modules)

For tasks with clearly separable components:

```
Seed Task
    │
    ├─▶ Agent 1: Module A (file: src/module_a.rs)
    ├─▶ Agent 2: Module B (file: src/module_b.rs)
    ├─▶ Agent 3: Module C (file: src/module_c.rs)
    │
    ▼
  Agent 4: Integrate + Verify (depends on all)
    │
    ▼
  Done
```

**When to use:** Task explicitly requires multiple independent files or components.
**Critical rule:** Agents MUST NOT modify the same files. List file scope in task descriptions.

### 5.4 Pattern D: Iterative Refinement Cycle

For tasks where the first attempt is unlikely to pass:

```
Seed Task
    │
    ▼
  ┌──────────────────┐
  │ Agent: Implement  │◀─── (max_iterations=3, cycle_delay=10s)
  │   + run tests     │
  │   + fix failures  │
  └────────┬─────────┘
           │
     ──converged──▶ Done
```

**When to use:** Complex tasks where iterative refinement is expected.
**Convergence signal:** `wg done --converged` when tests pass. `wg fail` to trigger retry with fresh agent.

### 5.5 Pattern E: Parallel Research Fan-Out (Future, Requires Web Tools)

For hard tasks where multiple solution approaches should be explored:

```
Seed Task
    │
    ├─▶ Agent 1: Research approach A (web_search)
    ├─▶ Agent 2: Research approach B (web_search)
    ├─▶ Agent 3: Research approach C (web_search)
    │
    ▼
  Agent 4: Synthesize findings → pick best approach
    │
    ▼
  Agent 5: Implement chosen approach
    │
    ▼
  Done
```

**When to use:** Task has multiple viable approaches, unclear which is best.
**Prerequisite:** Web tools in native executor (P1 implementation).
**Note:** Not available in initial Phase 3 deployment. Reserved for future iteration.

### 5.6 Coordinator Pattern Selection

The coordinator chooses patterns based on task characteristics:

| Task Signal | Pattern |
|-------------|---------|
| Simple implementation, clear instructions | A (pipeline) |
| Unfamiliar technology mentioned | B (research → implement) |
| Multiple independent files/modules required | C (parallel impl) |
| Hard task, likely needs iteration | D (refinement cycle) |
| Multiple viable approaches, unclear best | E (parallel research) — future |

The coordinator heartbeat prompt should include this decision framework.

---

## 6. Implementation Plan

### Phase 0: Immediate (No Code Changes)

**Goal:** Validate heartbeat concept with Option B (external `wg msg` loop).

| Step | Action | Effort |
|------|--------|--------|
| 0.1 | Write `heartbeat.sh` script that sends `wg msg send .coordinator-0 "HEARTBEAT: review and dispatch"` every 30s | 15 min |
| 0.2 | Add research hints to hard TB task descriptions (COBOL, Cython, constraint tasks) | 30 min |
| 0.3 | Update `adapter.py` Condition G config: set `autopoietic=False`, add coordinator agent setup | 30 min |
| 0.4 | Run 5-task smoke test with heartbeat loop | 1 hour |
| 0.5 | Measure: Does coordinator respond usefully to heartbeats? | Analysis |

**Deliverable:** Smoke test results showing whether heartbeat-driven coordination improves on fire-and-forget.

### Phase 1: Integrated Heartbeat (Option A Implementation)

**Goal:** Built-in heartbeat support in the wg daemon.

| Step | Action | Files Modified | Effort |
|------|--------|---------------|--------|
| 1.1 | Add `heartbeat_interval: u64` to `CoordinatorConfig` | `src/config.rs` | Small |
| 1.2 | Track `last_heartbeat` timestamp in daemon loop | `src/commands/service/mod.rs` | Small |
| 1.3 | When interval elapses, push synthetic `ChatRequest` to coordinator agent channel | `src/commands/service/mod.rs` | Medium |
| 1.4 | Template the heartbeat prompt (tick number, elapsed time, budget) | `src/commands/service/coordinator_agent.rs` | Small |
| 1.5 | Log heartbeat responses to daemon log | `src/commands/service/coordinator_agent.rs` | Small |
| 1.6 | Add `wg config --heartbeat-interval <seconds>` CLI support | `src/commands/config.rs` | Small |

**Estimated scope:** ~50-80 lines of Rust code in daemon loop + config.

### Phase 2: Web Tools in Native Executor

**Goal:** Enable agents to search the web for documentation and solutions.

| Step | Action | Files Modified | Effort |
|------|--------|---------------|--------|
| 2.1 | Create `src/executor/native/tools/web.rs` with `web_search` tool | New file | Medium |
| 2.2 | Implement `web_fetch` tool (HTTP GET + content extraction) | Same file | Medium |
| 2.3 | Register web tools in `ToolRegistry::default_all()` | `src/executor/native/tools/mod.rs` | Small |
| 2.4 | Add bundle filtering (research bundle includes web; bare bundle excludes) | `src/executor/native/bundle.rs` | Small |
| 2.5 | Add "Available Capabilities" prompt section (conditional on tools) | `src/service/executor.rs` | Small |

**Estimated scope:** ~200-400 lines of Rust code.

### Phase 3: Prompt Improvements

**Goal:** Teach agents to research before implementing.

| Step | Action | Files Modified | Effort |
|------|--------|---------------|--------|
| 3.1 | Add "Understand the problem" Step 0 to Required Workflow | `src/service/executor.rs` | Small |
| 3.2 | Add coordinator dispatch strategy to coordinator prompt | `src/commands/service/coordinator_agent.rs` | Small |
| 3.3 | Update Condition G meta-prompt (or remove if coordinator handles it) | `terminal-bench/wg/adapter.py` | Small |

### Phase 4: Validation

**Goal:** Statistical comparison of Phase 3 vs Phase 2 vs baselines.

| Step | Action |
|------|--------|
| 4.1 | Run 10-task matched comparison: G-Phase2 vs G-Phase3, 3 reps each |
| 4.2 | Measure: pass rate, completion time, heartbeat action rate, token cost |
| 4.3 | If Phase 3 > Phase 2: launch full 89-task run (5 reps) |
| 4.4 | Compare against A and F baselines from ongoing runs |

### Implementation Dependency Graph

```
Phase 0 (validate concept)
    │
    ├──▶ Phase 1 (integrated heartbeat)  ──┐
    │                                       │
    ├──▶ Phase 2 (web tools)  ─────────────┤
    │                                       │
    └──▶ Phase 3 (prompt improvements)  ───┤
                                            │
                                            ▼
                                     Phase 4 (validation)
```

Phases 1, 2, and 3 are independent and can be implemented in parallel. Phase 4 depends on all three.

---

## 7. Risk Analysis

### 7.1 Heartbeat Cost Overrun

**Risk:** 60 heartbeats × ~5K tokens/turn = ~300K tokens per 30-minute trial. At Sonnet pricing, ~$0.30/run. Over 445 trials (5 reps × 89 tasks), that's ~$130 in coordinator overhead alone.

**Mitigation:**
- Use Haiku for heartbeat turns (drops to ~$13 total). Coordinator reasoning for NOOP checks doesn't need Sonnet-level capability.
- Implement NOOP detection: if 5 consecutive heartbeats return NOOP, increase interval to 120s.
- Set a per-trial heartbeat budget (e.g., max 30 heartbeats).

### 7.2 Coordinator Latency Blocks Spawning

**Risk:** If heartbeat turns take 10-30s (LLM response time), the coordinator agent is busy during the tick and can't process real `GraphChanged` events.

**Mitigation:**
- Option A uses async channels — heartbeat is a message on the coordinator agent's queue, not blocking the tick loop.
- The tick phases (cleanup, spawn) run independently of the coordinator agent.
- Worst case: a 30s delay in strategic response, but mechanical spawning continues.

### 7.3 Coordinator Model Cost vs Task Model

**Risk:** Using Sonnet for the coordinator + M2.7 for task agents creates a two-model system. The Sonnet coordinator might create task descriptions that M2.7 can't follow.

**Mitigation:**
- The coordinator's job is strategic (what to do), not tactical (how to do it).
- Task descriptions should be simple and concrete — the coordinator isn't writing code.
- Monitor for "coordinator created task X, agent failed to understand it" patterns.
- Fallback: use M2.7 for coordinator too, accepting less strategic reasoning.

### 7.4 Heartbeat Adds No Value (High NOOP Rate)

**Risk:** Like Condition F surveillance (0 activations across 95 trials), heartbeats may produce only NOOPs if agents are generally reliable.

**Mitigation:**
- This is the **validation question**, not a blocker. Phase 0 explicitly tests this.
- Unlike surveillance (which only checked pass/fail on single tasks), heartbeats see the full graph and can make strategic decisions.
- Even a 90% NOOP rate means 6 useful interventions per 30-minute trial.
- If NOOP rate > 95% across the smoke test, abort and save the cost.

### 7.5 Web Tool Quality / Reliability

**Risk:** DuckDuckGo HTML API may be rate-limited, blocked, or return low-quality results inside Docker/CI environments.

**Mitigation:**
- Phase 2 is independent — if web tools don't work, Phases 1 and 3 still provide value.
- Implement a fallback: if `web_search` fails, log a warning and continue (don't fail the task).
- TB tasks are currently self-contained; web search is a nice-to-have enhancement, not a requirement.

### 7.6 Prompt Competition Persists

**Risk:** Even with the coordinator handling orchestration, individual task agents still receive the `REQUIRED_WORKFLOW` prompt. If the coordinator's task descriptions compete with this workflow, agents may be confused.

**Mitigation:**
- Phase 3 removes the autopoietic meta-prompt, eliminating the primary source of prompt competition.
- The coordinator creates standard tasks (not self-referential "build your own graph" tasks).
- Task agents follow `REQUIRED_WORKFLOW` normally — there's no conflict because they're just implementing, not orchestrating.

### 7.7 Max Agents Contention

**Risk:** With `max_agents=8`, the coordinator might create more tasks than can run concurrently, leading to resource contention or long queues.

**Mitigation:**
- The coordinator sees active agent count in the context snapshot.
- Heartbeat prompt includes: "Current agents: N/8 slots used" — coordinator can throttle task creation.
- The daemon's spawn logic already respects `max_agents`.

---

## 8. Success Criteria

| Metric | Target | Measurement |
|--------|--------|-------------|
| Pass rate (Phase 3) | >64% on TB 2.0 (beat Phase 2 best) | 10-task matched comparison |
| Heartbeat action rate | >5% (at least 1 useful action per trial) | Count non-NOOP heartbeats |
| Time to completion | <Phase 2 (agents worked until timeout) | Wall-clock per trial |
| Token overhead | <2x Phase 2 coordinator tokens | Measure heartbeat token usage |
| No regressions | A and F baselines unchanged | Control trials |

### Go / No-Go Decision Points

1. **After Phase 0 (smoke test):** If coordinator never takes useful actions on heartbeats, abort Phase 3 design. Consider returning to Phase 2 with targeted fixes.
2. **After Phase 1 (integrated heartbeat):** If pass rate ≤ Phase 2 on 10-task comparison, investigate whether the coordinator model matters (try Opus vs Sonnet vs Haiku).
3. **After Phase 4 (full validation):** If pass rate improvement < 10pp over Phase 2, the heartbeat overhead may not be worth the cost. Publish results either way.

---

## 9. Source References

| Document | Path | What it provides |
|----------|------|-----------------|
| Condition G status | `terminal-bench/docs/research-condition-g-status.md` | G definition, evolution, trial results, blockers |
| Coordinator heartbeat research | `terminal-bench/docs/research-coordinator-heartbeat.md` | Architecture, gap analysis, implementation options |
| Web search prompting research | `terminal-bench/docs/research-agent-web-prompting.md` | Tool availability, prompt gaps, fan-out patterns |
| Adapter implementation | `terminal-bench/wg/adapter.py` | Condition configs, meta-prompts, executor setup |
| Daemon event loop | `src/commands/service/mod.rs:2483` | Where heartbeat timer would be added |
| Coordinator tick | `src/commands/service/coordinator.rs:3435` | Tick phases (mechanical, no LLM) |
| Context injection | `src/commands/service/coordinator_agent.rs:2400` | `build_coordinator_context()` |
| Native tool registry | `src/executor/native/tools/mod.rs:211` | Where web tools would be registered |
| Prompt builder | `src/service/executor.rs:675` | `build_prompt()` sections |
| Scale experiment design | `terminal-bench/docs/scale-experiment-design.md` | A vs G experimental framework |
| Pilot results | `terminal-bench/docs/pilot-results-synthesis.md` | A vs F pilot data |
| Surveillance audit | `terminal-bench/docs/surveillance-audit.md` | Why surveillance added 0 value |
