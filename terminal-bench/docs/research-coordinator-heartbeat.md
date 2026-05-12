# Research: Coordinator Heartbeat Pattern for TB Orchestration

**Date:** 2026-04-08
**Task:** research-coordinator-heartbeat
**Status:** Research complete — implementation options proposed

---

## 1. Executive Summary

The user observes that conversational interaction with the coordinator agent produces excellent orchestration — the coordinator sees full context, makes good dispatch decisions, and handles failures. The question: **how do we replicate this for autonomous Terminal Bench runs?**

This research documents the current coordinator architecture, identifies the gap between interactive and autonomous modes, analyzes prior attempts (Condition F surveillance), and proposes three implementation options for a "forced heartbeat" pattern.

**Key finding:** The interactive coordinator's advantage comes from **per-turn context injection** — on each user message, it receives a fresh `build_coordinator_context()` snapshot with graph summary, recent events, active agents, and attention items. The service daemon's tick loop (which spawns agents) is **not LLM-driven** — it's pure Rust code that mechanically checks readiness and spawns. The heartbeat idea bridges this gap by synthetically generating coordinator turns on a timer.

---

## 2. Current Coordinator Architecture

### 2.1 Service Daemon Overview

The daemon (`wg service start`, `src/commands/service/mod.rs:2200`) runs a single event loop:

```
while running:
    reap_zombies()
    poll(listener_fd, timeout)    # epoll-based, not busy-spin
    handle_ipc_connections()      # GraphChanged, UserChat, Reconfigure, etc.
    if should_tick:
        coordinator_tick()        # Phase 1–6: cleanup, cycles, spawning
```

**Three tick triggers** (in priority order):
1. **Urgent wake** — `UserChat` IPC arrives → immediate tick (bypasses settling/pause)
2. **Settling deadline** — `GraphChanged` IPC arrives → tick after settling delay (default 2000ms, debounced)
3. **Background safety net** — `poll_interval` timer (default 60s) fires even without IPC events

### 2.2 Coordinator Tick (`coordinator.rs:3435`)

The tick is **pure Rust, no LLM involvement**. It runs these phases:

| Phase | What it does | Line ref |
|-------|-------------|----------|
| 1 | Clean up dead agents, reconcile orphans, kill zombies | `cleanup_and_count_alive()` |
| 1.3 | Zero-output agent detection (5min+ with no stdout) | `sweep_zero_output_agents()` |
| 1.5 | Auto-checkpoint alive agents | `auto_checkpoint_agents()` |
| 2.5 | Cycle iteration (reactivate done cycles) | `evaluate_all_cycle_iterations()` |
| 2.6 | Cycle failure restart | `evaluate_all_cycle_failure_restarts()` |
| 2.7 | Evaluate waiting tasks (wait conditions) | `evaluate_waiting_tasks()` |
| 2.8 | Message-triggered resurrection | `resurrect_done_tasks()` |
| 3 | Auto-assign unassigned ready tasks (agency LLM call) | `build_auto_assign_tasks()` |
| 4 | Auto-evaluate completed tasks (agency LLM call) | `build_auto_evaluate_tasks()` |
| 4.5 | FLIP verification | `build_flip_verification_tasks()` |
| 5 | Check for ready tasks | `check_ready_or_return()` |
| 6 | Spawn agents on ready tasks | `spawn_agents_for_ready_tasks()` |

**Critical observation:** The tick handles *graph maintenance and spawning* but does **no strategic reasoning** about task priority, failure recovery strategy, or adaptive re-planning. It spawns whatever is ready.

### 2.3 Coordinator Agent (`coordinator_agent.rs`)

The coordinator agent is a **separate, persistent LLM session** (Claude CLI subprocess). It is:

- Spawned on daemon startup (or lazily on first `UserChat`)
- Long-lived — survives across many user interactions
- Fed messages via `mpsc` channel from the daemon's IPC handler
- Auto-restarted on crash (rate-limited: max 3 restarts per 10 minutes)

**Per-turn context injection** (the key to its effectiveness):

On each user message, `build_coordinator_context()` (`coordinator_agent.rs:2400`) injects:

1. **Compacted project context** — `context.md` from the compactor (project-level knowledge)
2. **Conversation context summary** — compacted prior conversation history
3. **Injected history context** — user-selected prior conversation snippets
4. **Graph summary** — `N tasks: X done, Y in-progress, Z open, W blocked, V failed`
5. **Recent events** — last 20 events from the `EventLog` ring buffer (task completions, agent spawns, failures)
6. **Active agents** — which agents are running on which tasks + uptime
7. **Attention needed** — failed tasks with reasons

This context snapshot is prepended to the user's message before sending to the LLM.

### 2.4 Configuration Defaults

| Parameter | Default | Config key | Source |
|-----------|---------|-----------|--------|
| `poll_interval` | 60s | `coordinator.poll_interval` | Background safety-net timer |
| `interval` | 30s | `coordinator.interval` | Standalone coordinator tick interval |
| `settling_delay_ms` | 2000ms | `coordinator.settling_delay_ms` | Debounce for GraphChanged bursts |
| `max_agents` | 8 | `coordinator.max_agents` | Parallel agent limit |
| `coordinator_agent` | true | `coordinator.coordinator_agent` | Enable persistent LLM coordinator |

### 2.5 IPC Wake Mechanisms

The daemon already supports several IPC requests that can wake the coordinator:

- `GraphChanged` — sets a settling deadline, then ticks
- `UserChat { coordinator_id, request_id }` — urgent wake, routes to coordinator agent
- `Heartbeat { agent_id }` — records agent heartbeat (doesn't trigger tick)
- `Reconfigure { ... }` — updates daemon config (doesn't trigger tick)

There is **no "force tick" or "synthetic coordinator prompt"** IPC command today.

---

## 3. Gap: Interactive vs. Service Coordinator

### 3.1 What the Interactive Coordinator Sees

When a user sends a message via `wg chat`, the coordinator agent receives:

```
## System Context Update (2026-04-08T19:00:00Z)

### Compacted Project Context
[Full project knowledge from context.md]

### Conversation Context Summary
[Prior conversation summary]

### Graph Summary
45 tasks: 30 done, 5 in-progress, 3 open, 2 blocked, 3 failed, 2 abandoned

### Recent Events
- [18:55:02] task auth-impl completed (agent-12345)
- [18:56:10] task auth-test failed: test_auth_expired_token assertion error
- [18:57:00] agent agent-12350 spawned on auth-integration (executor: claude)

### Active Agents
- agent-12345 working on "db-migration" (uptime: 12m)
- agent-12350 working on "auth-integration" (uptime: 3m)

### Attention Needed
- FAILED: auth-test "Auth token expiry test" — test_auth_expired_token assertion error

---

User message:
The auth tests are failing, what's going on?
```

The LLM can reason about the failure, check which agent worked on it, suggest next steps, and dispatch new tasks.

### 3.2 What the Service Tick Does

The tick loop operates entirely without LLM reasoning:

1. Dead agents? → Unclaim their tasks
2. Cycles need iteration? → Reset them
3. Ready tasks? → Spawn agents on them
4. That's it.

**No strategic reasoning.** No "this task failed twice, maybe the approach is wrong." No "these three tasks are related, dispatch them in sequence." No "the auth tests failed because auth-impl is incomplete, hold off on auth-test until auth-impl is re-done."

### 3.3 The Gap

| Capability | Interactive | Service Tick |
|-----------|------------|-------------|
| Context injection (graph + events + agents) | Yes, per turn | No |
| LLM reasoning about strategy | Yes | No |
| Adaptive failure handling | Yes (can re-plan) | Mechanical (restart cycles) |
| Task priority reasoning | Yes | FIFO/ready-order |
| Cross-task awareness | Yes (sees graph summary) | Limited (cycle/dependency only) |
| Progress monitoring | Yes (event log) | Alarm-only (zero-output, zombie) |
| User interaction | Yes | No |
| Cost per invocation | ~$0.01–0.10 (LLM call) | ~$0 (pure Rust) |

**The heartbeat pattern aims to close this gap by synthetically generating coordinator agent turns on a timer, giving the LLM regular opportunities to observe and act on the graph state.**

---

## 4. Condition F: What It Tried and Why Surveillance Failed

### 4.1 Condition F Design

Condition F (`terminal-bench/analysis/condition-f-final-design.md`) added surveillance loops to each TB trial:

- Work agent completes task → mark done
- Surveillance agent spawns → runs verify command → confirms or triggers retry
- Cycle between work and surveillance (max 3 iterations, 1min delay)

### 4.2 Results

- **Pilot (89 tasks, 90 trials):** F achieved 98.9% pass rate vs A's 41.6%
- **Full sweep (7 tasks, 21 trials):** F achieved 100% pass rate (same as all other conditions — task set too easy)
- **Surveillance activations: 0 across 95 trials**

### 4.3 Why Surveillance Added Zero Value

From `terminal-bench/docs/surveillance-audit.md`:

1. **The model was too reliable.** At 98.9% first-attempt pass rate, there were essentially no failures for surveillance to catch.
2. **Surveillance can't fix capability failures.** The one genuine failure (`iterative-test-fix-r1`, 1805s timeout) was a budget exhaustion — surveillance retry just wastes more budget.
3. **Surveillance is same-model verification.** The surveillance agent uses the same model and similar context. If the work agent didn't notice an issue, the surveillance agent likely won't either.
4. **Token overhead without benefit.** Surveillance added ~3.5x token cost for 0 activations.

### 4.4 Key Insight: Context Injection Was the Active Ingredient

The pilot report (`terminal-bench/docs/pilot-a89-vs-f89-report.md`) concluded:

> "Infrastructure acts as an intelligence multiplier. The same model, on the same tasks, goes from coin-flip reliability to near-perfect execution when embedded in a coordination framework. The improvement comes from context injection — the surveillance loop added zero value."

This led to Condition G: F minus surveillance = context injection only.

### 4.5 How the Heartbeat Idea Differs from Surveillance

| Aspect | Condition F Surveillance | Heartbeat Pattern |
|--------|------------------------|-------------------|
| What observes | Per-task companion agent | Central coordinator LLM |
| When it runs | After each task completes | On timer (e.g., 30s) |
| What it sees | Single task's output | Entire graph state |
| What it can do | Retry single task | Re-plan, create tasks, kill agents, adjust strategy |
| Cost model | Per-task (scales with N tasks) | Per-heartbeat (scales with time) |
| Strategic reasoning | None (binary pass/fail) | Full (LLM-based) |

The heartbeat is **fundamentally different** — it's a central strategist periodically reviewing the whole battlefield, not a per-unit inspector checking individual work items.

---

## 5. Implementation Options

### Option A: Tight Coordinator Agent Heartbeat (Recommended)

**Mechanism:** Add a synthetic "heartbeat" message to the coordinator agent on a configurable timer.

**How it works:**
1. In the daemon's main loop, track `last_heartbeat` timestamp
2. When `heartbeat_interval` (e.g., 30s) elapses, inject a synthetic `UserChat` message:
   ```
   [HEARTBEAT] Autonomous check-in. Review graph state, dispatch new work,
   handle failures, and report any issues. If no action needed, respond briefly.
   ```
3. The existing `build_coordinator_context()` prepends the full graph snapshot
4. The coordinator agent responds with any actions (creates tasks, kills agents, etc.)
5. If the coordinator has bash tool access, it can run `wg add`, `wg kill`, etc.

**Implementation:**
- Add `heartbeat_interval: u64` to `CoordinatorConfig` (default: 0 = disabled, for TB: 30)
- In `mod.rs:2483` daemon loop, add heartbeat deadline tracking alongside `settling_deadline`
- When heartbeat fires, create a `ChatRequest` with synthetic message and push to coordinator agent's channel
- Log heartbeat responses to daemon log

**Pros:**
- Minimal code change (~50 lines in daemon loop + config)
- Reuses all existing infrastructure (coordinator agent, context injection, event log)
- Coordinator gets the same rich context as interactive mode
- Configurable per-project (TB can set 30s, normal projects can disable)

**Cons:**
- LLM cost: each heartbeat is a full coordinator turn (~$0.01–0.10 depending on model)
- For a 30-minute TB run with 30s heartbeat, that's ~60 heartbeat turns
- May generate unnecessary responses ("everything looks fine") on most ticks
- Coordinator agent must be enabled (already the default)

**Estimated cost for TB:**
- 60 heartbeats × ~5K tokens/turn = ~300K tokens per run
- At Sonnet pricing: ~$0.30/run; at Haiku: ~$0.03/run
- Compared to task agent tokens (~1M+ per run), this is 10–30% overhead

### Option B: External Heartbeat via `wg msg`

**Mechanism:** A separate cron-like process sends periodic `wg msg` to the coordinator task.

**How it works:**
1. Start a loop (shell script, Python, or `wg` built-in) that runs every N seconds:
   ```bash
   while true; do
     wg msg send .coordinator-0 "HEARTBEAT: Check status and dispatch"
     sleep 30
   done
   ```
2. The coordinator agent already monitors its inbox (the `Message` wait condition)
3. On receiving the message, the coordinator wakes and processes it

**Implementation:**
- No code changes to wg — uses existing `wg msg` and `Message` wait conditions
- TB runner script starts the heartbeat loop alongside `wg service start`
- Stop the loop when the run completes

**Pros:**
- Zero code changes to wg binary
- Can be tested immediately
- Easy to adjust interval without recompilation
- Works with any coordinator setup

**Cons:**
- Depends on coordinator task being in `Waiting` status with `Message` condition
- External process to manage (start/stop/health)
- Messages accumulate in the message log (minor)
- Less clean than integrated approach
- Coordinator must be configured to wake on messages

### Option C: Coordinator Heartbeat Mode (New Feature)

**Mechanism:** A new coordinator run mode where the LLM is prompted on every tick, not just on user messages.

**How it works:**
1. Add `coordinator.heartbeat_mode: bool` config flag
2. When enabled, every coordinator tick also triggers a coordinator agent turn
3. The tick prompt includes the full `build_coordinator_context()` plus a structured action menu:
   ```
   ## Heartbeat Tick #47 (30s since last)
   [context injection]

   Available actions:
   - Create tasks: wg add "..." --after ... --verify "..."
   - Kill stuck agents: wg kill <agent-id>
   - Adjust priority: wg msg send <task-id> "..."
   - Do nothing: respond "NOOP"
   ```
4. Parse the coordinator's response for actions

**Implementation:**
- Modify coordinator tick to optionally invoke coordinator agent
- Add structured prompting for heartbeat mode
- Parse and execute coordinator LLM responses
- Add config flag and CLI option

**Pros:**
- Most tightly integrated option
- Tick frequency matches daemon's native cadence
- Could evolve into the "coordinator reasons about every tick" pattern
- Opens door to LLM-driven priority/scheduling decisions

**Cons:**
- Most complex implementation (~200+ lines)
- Couples LLM invocation to the tick loop (latency concerns)
- Risk of coordinator tick becoming slow (LLM response time: 5–30s)
- Tick must not block on LLM — needs async handling

---

## 6. Comparative Analysis

| Criterion | Option A (Synthetic Chat) | Option B (External wg msg) | Option C (Heartbeat Mode) |
|-----------|--------------------------|---------------------------|--------------------------|
| **Implementation effort** | Small (~50 LOC) | None (shell script) | Large (~200+ LOC) |
| **Code changes** | Daemon loop only | None | Daemon + coordinator + config |
| **Rich context** | Yes (full context injection) | Partial (message only, no context snapshot) | Yes (full context injection) |
| **Latency risk** | Low (async via channel) | Low (separate process) | Medium (tick blocks on LLM) |
| **TB integration** | Config flag in trial setup | Script alongside runner | Config flag in trial setup |
| **Testability** | Immediate (enable flag) | Immediate (run script) | After implementation |
| **Future value** | Reusable for any project | Ad-hoc | Foundation for LLM-driven scheduling |

### Recommendation

**Start with Option B for immediate testing, then implement Option A for production.**

Option B requires zero code changes — the TB runner can start a heartbeat loop today and measure whether periodic coordinator wake-ups improve autonomous performance. If the pattern proves valuable, Option A provides a clean, integrated implementation with full context injection.

Option C is the most powerful but should wait until Options A/B validate the pattern. It's premature to couple the tick loop to LLM invocation without evidence that heartbeats add value.

---

## 7. TB-Specific Heartbeat Design

For Terminal Bench autonomous runs, the heartbeat would work as follows:

### 7.1 Setup

```toml
# .wg/config.toml
[coordinator]
coordinator_agent = true
heartbeat_interval = 30  # seconds (Option A)
# Or use Option B: external heartbeat script
```

### 7.2 Heartbeat Prompt (Option A)

```
[AUTONOMOUS HEARTBEAT] Check #{{tick_number}} at {{timestamp}}

You are running autonomously (no human operator). Review the system state
and take any needed actions:

1. Are all agents healthy? (check for stuck/slow agents)
2. Are there failed tasks that need re-planning?
3. Are there ready tasks that should be prioritized?
4. Is progress on track? Any bottlenecks?

If everything is fine, respond with "NOOP — all systems nominal."
If you take actions, briefly log what and why.
```

### 7.3 Expected Behavior

In a typical TB run (5–10 tasks, 30-minute wall clock):
- ~60 heartbeats over 30 minutes
- Most heartbeats: NOOP (agents working normally)
- Valuable heartbeats: agent stuck → kill and retry; task failed → create follow-up; dependency chain stalled → unblock

### 7.4 Measurement

To evaluate heartbeat value, compare:
- **Condition G (current):** Context injection, no heartbeat, fire-and-forget agents
- **Condition G+H:** Context injection + coordinator heartbeat every 30s

Metrics:
- Pass rate (primary)
- Time to completion
- Failure recovery rate (how often heartbeat catches and fixes a problem)
- Token overhead (heartbeat cost)
- NOOP rate (what fraction of heartbeats take no action — high NOOP = wasted tokens)

---

## 8. Concrete Next Steps

1. **Immediate (Option B prototype):** Write a `heartbeat.sh` script that runs `wg msg send .coordinator-0 "HEARTBEAT: review and dispatch"` every 30s. Test with a 5-task TB run to measure whether the coordinator agent responds usefully.

2. **Short-term (Option A implementation):** Add `heartbeat_interval` config to `CoordinatorConfig` (default 0 = disabled). Track `last_heartbeat` in daemon loop. When interval elapses, push synthetic `ChatRequest` to coordinator agent channel.

3. **Validation:** Run matched G vs G+H comparison on 10 TB tasks × 3 reps. Measure pass rate, completion time, and heartbeat action rate.

4. **Iterate:** If heartbeat NOOP rate > 90%, increase interval to 60s or 120s. If heartbeat catches real issues, decrease to 15s. Cost-optimize by using a cheaper model for heartbeat turns.

---

## 9. Appendix: Source References

| Component | File | Key Lines |
|-----------|------|-----------|
| Daemon main loop | `src/commands/service/mod.rs` | 2483–2900 (event loop, tick triggers) |
| Coordinator tick | `src/commands/service/coordinator.rs` | 3435–3617 (Phase 1–6) |
| Context injection | `src/commands/service/coordinator_agent.rs` | 2400–2600 (`build_coordinator_context`) |
| System prompt | `src/commands/service/coordinator_agent.rs` | 2083–2109 (`build_system_prompt`) |
| Agent message loop | `src/commands/service/coordinator_agent.rs` | 580–730 (per-turn processing) |
| Config defaults | `src/config.rs` | 2122–2410 (`CoordinatorConfig`) |
| IPC requests | `src/commands/service/ipc.rs` | 25–80 (`IpcRequest` enum) |
| Event log | `src/commands/service/coordinator_agent.rs` | 66–255 (`EventLog` ring buffer) |
| Condition F results | `terminal-bench/analysis/condition-f-results.md` | Full sweep analysis |
| Surveillance audit | `terminal-bench/docs/surveillance-audit.md` | Why surveillance added 0 value |
| Pilot A vs F | `terminal-bench/docs/pilot-a89-vs-f89-report.md` | Matched-set comparison |
| Scale experiment design | `terminal-bench/docs/scale-experiment-design.md` | G vs A design |
