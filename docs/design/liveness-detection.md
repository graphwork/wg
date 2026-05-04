# Design: Liveness Detection for workgraph Agents

**Status:** Committee consensus (Researchers A-E)
**Date:** 2026-03-04
**Supersedes:** `docs/design/sleep-aware-liveness.md` (individual research)

## Problem

When a laptop hibernates/sleeps and resumes, agent connections may break, leaving processes that appear alive (PID exists) but are actually stuck. The current coordinator only detects dead agents by PID exit — it cannot detect an agent whose process is alive but hung due to a broken connection post-sleep.

Additionally, agents that block on dependencies waste slots and tokens by keeping a process alive with no productive work to do.

## Committee Participants

| Researcher | Focus Area |
|-----------|------------|
| A | Linux sleep/wake detection methods (CLOCK_MONOTONIC, systemd-logind, timerfd, cross-platform) |
| B | Existing coordinator detection code, stuck agent heuristics, industry patterns |
| C | Lightweight checker agent design, K8s probes, circuit breakers |
| D | Agent hibernation (`wg wait`), context replay via JSONL, `claude --resume` |
| E | Long-running agent cost/efficiency, parking economics, distributed system patterns |

## Consensus Design

### 1. Sleep-Aware Detection Algorithm

**Core mechanism: Monotonic clock drift.**

Compare `libc::clock_gettime(CLOCK_MONOTONIC)` against `SystemTime::now()`. CLOCK_MONOTONIC pauses during system sleep while the wall clock advances. A divergence > 30 seconds indicates the system was asleep.

```rust
fn monotonic_secs() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts); }
    ts.tv_sec as f64 + ts.tv_nsec as f64 / 1_000_000_000.0
}
```

**Why libc directly instead of `std::Instant`:** There are active PRs to switch Rust's `Instant` from CLOCK_MONOTONIC to CLOCK_BOOTTIME (which does NOT pause during sleep). Pinning to `libc::clock_gettime(CLOCK_MONOTONIC)` directly insulates us from std changes. We already depend on libc for `kill(pid, 0)`, so this adds zero new dependencies. (Consensus: A, B, C all agreed on this approach.)

**Why not systemd-logind, timerfd, or IOKit:** Researcher A evaluated all alternatives. systemd-logind has an unreliable resume signal (systemd issue #30666). timerfd is Linux-only and more complex for no benefit. IOKit requires unsafe FFI on macOS. The clock drift approach is simpler, more portable, and sufficient — retroactive detection on the next coordinator tick is all we need.

**Algorithm per coordinator tick:**

1. Compute `wall_elapsed` (SystemTime diff) and `mono_elapsed` (CLOCK_MONOTONIC diff) since last tick
2. `sleep_gap = wall_elapsed - mono_elapsed`
3. If `sleep_gap > 30s`: system slept. Log the gap. Set grace period (2 min). Reset stale counters.
4. If within grace period: skip stuck-agent checks (agents may need time to reconnect)
5. If past grace period and no recent sleep: proceed with stuck-agent detection

**Data structure** (lives in daemon main loop, not persisted):

```rust
struct SleepTracker {
    last_tick_wall: SystemTime,
    last_tick_mono: f64,  // from monotonic_secs()
    wake_grace_until: Option<Instant>,
    agent_stale_ticks: HashMap<String, u32>,  // agent_id → consecutive stale tick count
}
```

### 2. Stuck Agent Detection

**Current state (what's broken):**
- Stream staleness check exists (`STREAM_STALE_THRESHOLD_MS = 5 min` in `triage.rs:29`) but only logs a warning — takes no action
- Heartbeat auto-bump at `triage.rs:92-94` makes heartbeats useless: coordinator bumps heartbeat for any PID-alive agent, masking stuck agents
- No mechanism to intervene when an agent is stuck-but-alive

**Fix: Remove heartbeat auto-bump.** Stream events (Turn, ToolStart, ToolEnd, Heartbeat from executor) are the ground truth liveness signal. The coordinator should read `stream.jsonl` timestamps, not fabricate heartbeats. (Unanimous consensus.)

**Stuck detection flow:**

1. For each InProgress task with an alive PID (Waiting/Parked tasks are excluded):
2. Read last event timestamp from `stream.jsonl`
3. If `now - last_event > stale_threshold` (default: 10 min, measured in awake-time only):
   - **Check for in-progress tools:** If the last event is `ToolStart` without a matching `ToolEnd`, the agent may be in a long-running tool call (e.g., `cargo build`). Extend the window — do not count this tick as stale.
   - Otherwise, increment `agent_stale_ticks[agent_id]`
4. If `agent_stale_ticks >= 2` (K8s `failureThreshold` pattern): trigger stuck triage
5. If agent produces any stream event: reset `agent_stale_ticks` to 0

**Why 10 minutes (not 5):** Researcher E's cost analysis showed false positive asymmetry is 10-50x. Killing a working agent costs $1-5 + 5-15 min wasted. Letting a stuck agent sit 5 extra minutes costs ~$0 + one blocked slot. Combined with the 2-tick requirement (failureThreshold), effective minimum detection time is ~12 minutes. Conservative enough to avoid false positives, responsive enough for real stuck agents.

**Linux-only refinements (optional, Phase 3):**
- `/proc/<pid>/stat`: Check process state (Z=zombie, T=stopped → treat as dead immediately)
- `/proc/<pid>/io`: Check if read/write bytes changed between ticks (confirms true I/O stall)
- These are refinements, not requirements. The core algorithm (stream staleness + clock drift) works cross-platform.

### 3. Lightweight Checker (Stuck Triage)

**Architecture:** Extend the existing `triage.rs` system. The checker is a synchronous haiku call from the coordinator — NOT a separate agent process. This reuses the proven `run_triage()` pattern. (Unanimous consensus.)

**New enum variant:**

```rust
enum DeadReason {
    ProcessExited,
    StuckAlive { stale_duration_secs: u64, last_tool: Option<String> },
}
```

**Checker prompt includes:**
- Output log tail (last 50KB, via existing `read_truncated_log`)
- Task description (title, id)
- Staleness duration (how long since last stream event)
- Last in-progress tool name (if any ToolStart without ToolEnd)
- Process state (if available from /proc on Linux)

**3 verdicts for stuck-alive agents:**

| Verdict | Action |
|---------|--------|
| **wait** | Agent likely still working (long build, etc.). Reset stale counter, check again later. |
| **kill-done** | Agent finished but hung on cleanup. SIGTERM → SIGKILL, mark task Done. |
| **kill-restart** | Agent is truly stuck. SIGTERM → SIGKILL, mark task Open for reassignment. |

**Dead-agent triage keeps its existing 3 verdicts unchanged:** done / continue / restart.

**Notification is orthogonal:** The coordinator can be configured to notify on any stuck detection event (via existing HITL notification channels) regardless of the checker's verdict. This keeps the checker focused on operational decisions. (Consensus reached after initial disagreement between B and C on whether "escalate" should be a verdict.)

**Retry cooldown:** After a stuck-triage intervention (kill-restart), enforce a configurable per-task cooldown (default: 60s) before the coordinator reassigns the task. Prevents rapid retry loops / thrashing.

**Circuit breaker (future):** Track failure rate per task. After N consecutive failures on the same task, stop retrying (existing `max_retries` in `apply_triage_verdict`). Add backoff between retries.

### 4. Configuration

New fields in `[agent]` section of `config.toml`:

```toml
[agent]
# Existing
heartbeat_timeout = 5       # minutes — used by wg dead-agents CLI

# New — liveness detection
stale_threshold = 10         # minutes of awake-time with no stream activity
wake_grace_period = 2        # minutes after wake before checking liveness
sleep_gap_threshold = 30     # seconds of wall-vs-mono divergence to detect sleep
stale_tick_threshold = 2     # consecutive stale ticks before triggering triage
retry_cooldown = 60          # seconds after triage intervention before reassigning
```

### 5. Integration Plan

**Phase 1 — Detection + Logging (ship first):**
- Add `SleepTracker` to daemon main loop (`src/commands/service/mod.rs`)
- Implement monotonic clock drift detection (sleep gap)
- Grace period logic (skip checks for 2 min after wake)
- Remove heartbeat auto-bump at `triage.rs:92-94`
- Promote stream staleness from warning to tracked counter (stale_ticks)
- Log all detections but take NO action yet
- ~80 lines of new code
- Validates the heuristic with real data before enabling intervention

**Phase 2 — Stuck Triage + Kill:**
- Add `StuckAlive` variant to `DeadReason`
- Implement `build_stuck_triage_prompt()` with enriched context
- Extend `cleanup_dead_agents()` to handle stuck-alive case
- Apply verdict: wait (reset counter), kill-done (SIGTERM→SIGKILL + Done), kill-restart (SIGTERM→SIGKILL + Open)
- Retry cooldown per task
- Gate behind existing `auto_triage` config flag
- ~100 lines of new code

**Phase 3 — Refinements:**
- Linux `/proc/<pid>/io` and `/proc/<pid>/stat` checks
- `wg dead-agents --check-stuck` CLI flag for on-demand inspection
- ToolStart-without-ToolEnd window extension logic
- Failure rate tracking per task (circuit breaker)

**Phase 4 — Parking & Resume (future, depends on `wg wait` implementation):**
- New `Waiting` task state (agent-initiated, auto-resolvable)
- `wg wait <task-id>` command: agent exits cleanly, coordinator stores session_id
- Resume via `claude --resume <session-id>` for Claude executor (zero replay cost, full context preservation server-side)
- Resume via checkpoint summary injection for non-Claude executors (generalize existing `## Previous Attempt Recovery` pattern from triage)
- **Always save checkpoint summary at parking time** as fallback for expired Claude sessions (belt-and-suspenders, per Researcher D)
- Inject brief graph state delta on resume (not full `wg context` dump — token accumulation concern per Researcher E)
- `AgentStatus::Parked` does NOT count against `max_agents`

### 6. Platform Considerations

| Feature | Linux | macOS |
|---------|-------|-------|
| Monotonic clock drift | `libc::CLOCK_MONOTONIC` vs `SystemTime` | Same (macOS monotonic pauses during sleep) |
| `/proc/<pid>/io` I/O check | Yes | No — skip (stream staleness sufficient) |
| `/proc/<pid>/stat` state | Yes | No — skip |
| `kill(pid, 0)` PID check | Yes | Yes |
| SIGTERM/SIGKILL | Yes | Yes |

The core algorithm (monotonic drift + stream staleness) is fully cross-platform. Linux-only `/proc` checks are optional refinements (Phase 3).

### 7. Failure Modes & Edge Cases

1. **Agent reconnects after sleep:** Grace period (2 min) allows recovery. If agent produces stream events during grace, it's fine.

2. **Long tool call (e.g., cargo build for 20 min):** ToolStart without matching ToolEnd extends the staleness window. The LLM checker can also reason about whether silence is normal for the reported tool.

3. **False positive — agent doing CPU work:** Stream events are produced regularly (turns, tool calls). 10+ minutes with zero events is a strong stuck signal. The 2-tick requirement adds another 1-2 min buffer.

4. **Coordinator itself sleeps:** The daemon detects the gap on its first tick after wake and applies grace period.

5. **Multiple rapid sleep/wake cycles:** Each wake resets the grace period. Agents that survive multiple cycles are likely fine.

6. **Race: agent finishes during triage:** Check task status after triage but before killing. If task is already Done/Failed, skip the kill.

7. **Claude session expiry:** Always save checkpoint summary as fallback. Try `--resume` first, fall back to summary-based reincarnation.

### 8. Cost Analysis (Researcher E)

| Strategy | Token cost (30 min wait) | Context quality | Slot consumed |
|----------|-------------------------|----------------|---------------|
| Keep alive (polling) | $1.80-$4.80 | Stale | YES |
| Kill + `--resume` (Claude) | ~$0 incremental | Preserved (server-side) | NO |
| Kill + reincarnate (summary) | $0.05-$0.12 | Fresh but lossy | NO |

False positive cost (killing working agent): $1-5 + 5-15 min wasted.
False negative cost (letting stuck agent sit 5 extra min): ~$0 + 1 blocked slot.
Asymmetry ratio: 10-50x. This justifies the conservative 10-min threshold.

### 9. Dissenting Opinions

**No major dissent.** All 5 researchers reached consensus on every point. Minor disagreements resolved during discussion:

- **Researcher C initially proposed 4 verdicts** (wait/done/restart/escalate). After Researcher B argued that "escalate" conflates detection with notification, C revised to 3 verdicts with notification as an orthogonal concern. All agreed.

- **Researcher E initially recommended reincarnation (kill + summary replay) as the universal resume strategy.** Researcher D showed that Claude `--resume` is strictly better for the Claude executor (zero replay cost, full context preservation). E revised to recommend `--resume` as primary with reincarnation as fallback. All agreed.

- **Researcher D raised concern about Claude session expiry.** All agreed on belt-and-suspenders: always save checkpoint summary, try `--resume` first, fall back to summary if session expired.

## References

- Existing triage: `src/commands/service/triage.rs`
- Coordinator main loop: `src/commands/service/mod.rs`
- Stream events: `src/stream_event.rs`
- Config: `src/config.rs` (`AgentConfig` struct)
- Prior research: `docs/design/sleep-aware-liveness.md`
