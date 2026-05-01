# Design: Unified Agent Lifecycle

**Status:** Committee consensus (Round 2 — Researchers A1, A2, B1, B2, C1, D1)
**Date:** 2026-03-04
**Builds on:** `docs/design/liveness-detection.md` (Round 1 — detection, stuck agents, sleep-aware monitoring)

## Problem

Agents in workgraph can stop for many reasons: they complete their task, they get stuck, the system sleeps, they need to wait for a dependency, or they receive a message after finishing. The previous design (liveness-detection.md) addressed detection and triage of stuck/dead agents. This document expands the design to cover the **full agent lifecycle** — a unified model where agents can stop and restart without losing context, regardless of why they stopped.

## The Unified Abstraction

**Core insight (D1):** All lifecycle transitions through a stopped state — waiting, resurrection, checkpoint recovery, stuck-kill-restart — use the **same checkpoint-to-resume pipeline**. The trigger differs; the resume path is identical.

```
Three triggers, one resume path:

  Agent-initiated ──→ wg wait (voluntary park)     ─┐
  Coordinator-initiated ──→ stuck detection (kill)   ├──→ checkpoint→resume pipeline
  External-triggered ──→ message on Done task        ─┘

Resume pipeline (executor-agnostic):
  1. Try executor-native session resume (zero/low cost, best context)
  2. Fall back to checkpoint summary injection (cheap, lossy)
```

This means every mechanism that stops an agent produces a **checkpoint**, and every mechanism that starts an agent consumes one. The checkpoint is the universal context bridge.

### Executor-Agnostic Design Principle

**Every mechanism in this design MUST work across all executor types.** The resume pipeline defines an abstract interface; each executor provides its own implementation. No feature may depend solely on Claude CLI capabilities.

| Executor | Session Resume | Checkpoint | Context Injection |
|----------|---------------|------------|-------------------|
| **Claude CLI** | `claude --resume <session_id>` (server-side context) | Stream event monitoring + auto-checkpoint | Prompt append via `--resume` follow-up |
| **Native (OpenAI-compatible)** | Conversation history replay from stored messages | Turn-count/timer triggers on stored conversation | Prepend checkpoint to system prompt on fresh spawn |
| **Amplifier** | Bundle-level session continuity | Delegated to sub-executor checkpoints | Bundle context injection |
| **Shell / Custom** | No session resume (always fresh) | No auto-checkpoint (explicit `wg checkpoint` only) | Checkpoint injected into task context template |

**The native executor is the primary target.** It manages its own conversation history in `.wg/agents/<id>/conversation.json`, making session resume a first-class capability without depending on any external service. The resume pipeline's fallback path (checkpoint injection) works for ALL executors, including shell and custom ones that have no session concept.

## Lifecycle States

### Task Status (extends existing enum)

```
                    ┌──────────────────────────────────────────────────┐
                    │                                                  │
                    ▼                                                  │
  ┌──────┐    ┌────────┐    ┌────────────┐    ┌──────┐               │
  │ Open │───→│InProgress│──→│   Done     │    │Failed│               │
  └──┬───┘    └────┬─────┘    └──┬───┬────┘    └──────┘               │
     │             │             │   │                                 │
     │             │  ┌──────────┘   │ (message arrives,              │
     │             │  │              │  no downstream running)         │
     │             │  │              ▼                                 │
     │             │  │     ┌────────────────┐                        │
     │             ▼  │     │ Done+child task │ (message arrives,     │
     │        ┌───────┤     │ parent stays   │  downstream running)   │
     │        │Waiting│     └────────────────┘                        │
     │        └───┬───┘                                               │
     │            │ (condition met)                                    │
     │            └───────────────────────────────────────────────────┘
     │
     ▼
  ┌───────┐
  │Blocked│  (structural — deps not met, auto-resolves)
  └───────┘
```

**Key additions to the existing model:**

| Status | Meaning | Agent process? | Slot consumed? |
|--------|---------|---------------|----------------|
| **Waiting** (new) | Agent voluntarily parked via `wg wait`. Condition stored. | No — agent exited | No |
| **InProgress** (existing, extended) | Agent actively working. May have `stuck_since` field set by coordinator. | Yes | Yes |
| **Done** (existing) | Task completed. May have unread messages triggering resurrection. | No | No |

**Stuck is a field, not a status.** An InProgress task with `stuck_since: Some(timestamp)` is being monitored by the liveness system (see liveness-detection.md). False-positive recovery is simply clearing the field when the agent produces new stream events.

**Paused remains an orthogonal boolean flag.** A paused task retains its underlying status (Open, InProgress, etc.) but is excluded from coordinator dispatch.

**Blocked vs Waiting:** Both have no running agent. Blocked is graph-structural (dependencies not met, resolved automatically when deps complete). Waiting is agent-initiated (voluntary park via `wg wait`, carries checkpoint + session_id + explicit condition). This semantic difference justifies a separate status.

## 1. Message-Triggered Resurrection

### When a message arrives on a Done task

**Detection:** On each coordinator tick, scan Done tasks for unread messages (messages with `status=Sent` that weren't sent by the task's own agent).

**Two resurrection modes** (conditional on downstream state):

1. **Reopen (preferred when safe):** If no downstream task has started execution (all downstream tasks are Open/Blocked), transition Done → Open. The coordinator's normal dispatch picks it up. Simpler, preserves session continuity.

2. **Child task (when downstream is running):** If any downstream task is already InProgress/Done, create a lightweight child task `respond-to-<parent-id>` that inherits the parent's `session_id`. The parent stays Done — no downstream confusion. The child prompt says: "You previously completed `<parent>`. Read and respond to pending messages."

**Session resume:** The child/reopened task resumes via the executor's native session mechanism (Claude CLI: `--resume <session_id>`, native executor: conversation history replay, etc.). If session resume fails or is unavailable for the executor type, fall back to checkpoint summary injection (belt-and-suspenders from Round 1).

**Batching:** Multiple messages on a Done task trigger ONE resurrection. The resurrected agent reads all pending messages via `wg msg read`.

**Guards:**
- Rate limit: max 5 resurrections per task, 60s cooldown between them
- Sender whitelist: only user, coordinator, or dependent-task agents can trigger resurrection
- Abandoned tasks: never resurrect
- `resurrect: false` tag or config to opt out per task

**Implementation detail — `completed_at`:** When reopening a Done task, keep `completed_at` (for timing analysis). Add `resurrected_at` timestamp to the log entry.

### Critical prerequisite: session persistence (executor-specific)

Session resume requires each executor to persist enough state to reconstruct conversation context:

| Executor | Persistence Mechanism | Action Required |
|----------|----------------------|-----------------|
| **Claude CLI** | Server-side sessions via `session_id` | Already implemented: `--resume` support exists in `execution.rs:385-390`. Ensure session IDs are stored on Task struct. |
| **Native** | Conversation history in `.wg/agents/<id>/conversation.json` | Already persisted by the native executor. Resume = replay stored messages to the API. |
| **Amplifier** | Delegated to sub-executor | Passes session context through bundle protocol. |
| **Shell / Custom** | None (stateless) | Checkpoint injection is the only resume path. |

Store `session_id` on the Task struct (not just AgentEntry) so it survives agent death and can be inherited by child tasks or successor agents. For the native executor, `session_id` maps to the conversation history file path. For Claude CLI, it maps to the server-side session. The field is executor-agnostic; interpretation is executor-specific.

## 2. Checkpointing for Long-Running Tasks

### The problem

Disk state (files, git commits, artifacts) survives agent death. The critical gap is **conversation context** — after hours of work, a dying agent loses all reasoning history. Task logs capture milestones but not the reasoning chain.

### Tiered approach (B2)

| Tier | Mechanism | Cost | Context quality | Executor support |
|------|-----------|------|----------------|-----------------|
| Free | Triage summary (existing) | $0 (already runs on death) | Lossy — post-mortem only | All |
| Cheap | Periodic checkpoint | ~$0.01/checkpoint (haiku) | Good — captures in-flight reasoning | All (auto or explicit) |
| Best | Executor-native session resume | $0 incremental (Claude), low (native replay) | Perfect — full context | Claude CLI, Native |

### Hybrid checkpointing (B1 + B2 consensus)

**Agent-driven (explicit):** Agents call `wg checkpoint <task-id> --summary "..."` at semantic boundaries (e.g., "finished research, starting implementation"). Higher quality summaries because the agent knows what matters.

**Coordinator-driven (auto-fallback):** The coordinator auto-generates checkpoints based on executor-reported progress:
- **Claude CLI:** Monitor stream events; trigger when `turn_count % N == 0` (default: N=15) or `time_since_last_checkpoint > M min` (default: M=20).
- **Native executor:** Tracks its own turn count and conversation length; emits checkpoint events directly or the coordinator queries its state via the agent process.
- **Shell / Custom:** Timer-based only (`time_since_last_checkpoint > M min`). No turn-count tracking. Explicit `wg checkpoint` is the primary path.

Works with any executor — timer fallback is always available.

**Both produce the same data structure:**

```json
{
  "task_id": "implement-feature-x",
  "agent_id": "agent-42",
  "timestamp": "2026-03-04T20:15:00Z",
  "type": "explicit|auto",
  "summary": "Completed research and initial implementation. Tests partially written. Remaining: edge case handling.",
  "files_modified": ["src/feature_x.rs", "tests/test_x.rs"],
  "artifacts_registered": ["src/feature_x.rs"],
  "stream_offset": 15234,
  "turn_count": 45,
  "token_usage": {"input": 50000, "output": 20000}
}
```

**Storage:** `.wg/agents/<agent-id>/checkpoints/<timestamp>.json`. Auto-prune to last 5 per task.

### Integration with triage "continue"

When triage produces a "continue" verdict for a task **with** checkpoints:
1. Skip the LLM-generated summary (checkpoint is better)
2. Inject latest checkpoint summary into `## Previous Attempt Recovery`
3. Include file list so successor agent knows what to check

Checkpoints are **complementary** to executor-native session resume. Session resume is primary (zero/low cost, full context). Checkpoint summary is the fallback when session resume fails (session expired, executor doesn't support resume, or executor changed between attempts). This is the belt-and-suspenders pattern from Round 1.

### Configuration

```toml
[checkpoint]
auto_interval_turns = 15    # auto-checkpoint every N turns
auto_interval_mins = 20     # or every N minutes, whichever comes first
max_checkpoints = 5         # keep only last N per task
```

## 3. `wg wait` — Agent-Initiated Parking

### Command

```
wg wait <task-id> --until <condition> [--checkpoint "summary of progress"]
```

**Semantics:** Park-and-exit, NOT block-and-wait. The agent exits after calling `wg wait`. The coordinator evaluates the condition on each tick and resumes the task when satisfied.

### Conditions

| Condition | Syntax | Example |
|-----------|--------|---------|
| Task completion | `task:<id>=done` | `task:research-api=done` |
| Timer | `timer:<duration>` | `timer:5m`, `timer:2h` |
| Human input | `human-input` | Wait for user message |
| Message | `message` | Wait for any message on this task |
| File existence | `file:<path>` | `file:src/config.rs` |

Composable: comma for AND (`task:a=done,task:b=done`), pipe for OR (`timer:5m|message`). No mixed AND/OR in v1.

### Data model

New fields on Task:
```rust
wait_condition: Option<WaitSpec>,   // what we're waiting for
session_id: Option<String>,         // executor-specific session reference
checkpoint: Option<String>,         // agent's summary at park time
```

New status: `Waiting`. New agent status: `Parked` (does NOT count against `max_agents`).

### Coordinator flow

On each tick, for each Waiting task:
1. Evaluate condition (all checks are O(1) — task status lookup, timer comparison, message scan)
2. If satisfied: clear `wait_condition` → set status to Open → normal dispatch picks it up
3. Spawn new agent with resume context: try executor-native session resume, fall back to checkpoint injection
4. Inject brief graph state delta (~100 tokens) showing what changed while waiting (NOT full `wg context` dump — token accumulation concern)

### Edge cases

- **Unsatisfiable condition** (waited-on task fails): Auto-fail the waiter with a descriptive message
- **Circular waits** (A waits on B, B waits on A): Detect via cycle analysis on wait edges, fail both
- **Agent doesn't exit after `wg wait`:** Grace period (30s), then SIGTERM
- **Session expiry:** Checkpoint fallback (always saved at park time)
- **Sequential waits:** Allowed — each `wg wait` overwrites the previous checkpoint. No concurrent waits (task can only be in one state)

## 4. Unified Resume Pipeline

All stopped→running transitions flow through the same pipeline:

```
┌─────────────────────┐
│   Trigger detected   │
│  (message/condition/ │
│   stuck-kill/death)  │
└──────────┬──────────┘
           ▼
┌─────────────────────┐
│  Gather resume       │
│  context:            │
│  - session_id        │
│  - latest checkpoint │
│  - pending messages  │
│  - graph state delta │
└──────────┬──────────┘
           ▼
┌─────────────────────┐     ┌─────────────────────┐
│ Try executor-native  │────→│ Success: agent has   │
│ session resume       │     │ full context         │
│ (Claude: --resume,   │     └─────────────────────┘
│  Native: history     │
│  replay, etc.)       │
└──────────┬──────────┘
           │ (fails — expired/unavailable/unsupported)
           ▼
┌─────────────────────┐     ┌─────────────────────┐
│ Fresh spawn with     │────→│ Agent gets:          │
│ checkpoint injection │     │ task desc + checkpoint│
│                      │     │ summary + file list  │
└─────────────────────┘     │ + graph delta        │
                            └─────────────────────┘
```

**What varies by trigger:**

| Trigger | Status transition | Retry count | Context injection |
|---------|------------------|-------------|-------------------|
| `wg wait` condition met | Waiting → Open | No increment | Checkpoint + delta |
| Message on Done task (safe) | Done → Open | No increment | Messages + checkpoint |
| Message on Done task (downstream running) | New child task | N/A | Messages + parent checkpoint |
| Stuck kill-restart | InProgress → Open | Increment | Checkpoint or triage summary |
| Agent death + continue | InProgress → Open | Increment | Checkpoint or triage summary |

### Executor-specific resume implementations

The `SessionResume` trait abstracts session resume across executors:

```rust
trait SessionResume {
    /// Attempt to resume a previous session. Returns modified spawn args
    /// if resume is possible, None if fallback to checkpoint is needed.
    fn try_resume(&self, session_id: &str, task: &Task) -> Option<SpawnArgs>;

    /// Inject checkpoint context into a fresh spawn (fallback path).
    fn inject_checkpoint(&self, checkpoint: &Checkpoint, spawn_args: &mut SpawnArgs);
}
```

| Executor | `try_resume` | `inject_checkpoint` |
|----------|-------------|---------------------|
| **Claude CLI** | Add `--resume <session_id>` to args; append checkpoint as follow-up message | Prepend checkpoint summary to prompt.txt |
| **Native** | Load `conversation.json`, replay messages to API, then inject new context | Prepend checkpoint to system prompt for new conversation |
| **Amplifier** | Forward resume request to sub-executor via bundle protocol | Inject into bundle context |
| **Shell/Custom** | Returns `None` (no session support) | Inject checkpoint into task context template via `{{checkpoint}}` variable |

**Key insight for the native executor:** Because it manages its own conversation history locally, session resume is always available (no server-side expiry risk). This makes it the most reliable executor for long-running lifecycle features. The conversation history file serves as both the session state AND the checkpoint source — the coordinator can extract the last N turns for checkpoint generation without an LLM call.

## Integration with Liveness Detection (Round 1)

This design **builds on** liveness-detection.md, not replaces it. The Round 1 design covers:
- Sleep-aware monotonic clock drift detection
- Stream staleness tracking (stale_ticks)
- Stuck agent triage (wait/kill-done/kill-restart verdicts)
- Phase 1-3 implementation plan for detection and intervention

This document adds:
- **Phase 4 expansion:** Parking (`wg wait`) and executor-native session resume
- **Phase 5:** Checkpointing system (auto + explicit)
- **Phase 6:** Message-triggered resurrection

The liveness detection system (Phases 1-3) feeds INTO this lifecycle:
- When stuck detection triggers a kill-restart, the **unified resume pipeline** handles the restart
- The checkpoint system provides better context for triage "continue" verdicts
- Parked agents (Waiting status) are **excluded** from stuck detection (they have no running process)

## Implementation Phases

### Phase 4: Session Persistence + `wg wait` (foundation)
- Define executor-agnostic `SessionResume` trait with per-executor implementations:
  - Claude CLI: `--resume <session_id>` (server-side context)
  - Native executor: conversation history replay from `.wg/agents/<id>/conversation.json`
  - Amplifier: delegate to sub-executor session mechanism
  - Shell/Custom: no-op (always falls back to checkpoint injection)
- Store `session_id` on Task struct and populate from executor-specific sources (Claude: stream.jsonl Init events, Native: conversation file path)
- Implement `wg wait` command (park-and-exit with condition)
- Add `Waiting` task status and `Parked` agent status
- Coordinator condition evaluation on tick
- Resume via executor's session mechanism + checkpoint fallback
- ~350 lines of new code (including executor trait + implementations)

### Phase 5: Checkpointing
- `wg checkpoint` CLI command for agents (works with all executor types)
- Coordinator auto-checkpoint trigger:
  - Claude CLI: stream event monitoring (turn count / time interval)
  - Native executor: query conversation length directly or receive checkpoint events
  - All executors: timer-based fallback
- Checkpoint storage and pruning
- Integration with triage "continue" (use checkpoint instead of LLM summary)
- `[checkpoint]` config section
- ~250 lines of new code

### Phase 6: Message Resurrection
- Coordinator scan for unread messages on Done tasks
- Conditional reopen vs child-task logic (based on downstream state)
- Session inheritance for child tasks
- Rate limiting and sender whitelist
- ~150 lines of new code

### Phase 7: Refinements
- Graph state delta injection on resume (compact summary of changes while parked)
- Pre-death checkpoint (coordinator requests checkpoint before SIGTERM)
- Resurrection exec_mode options (lightweight "answer-only" mode for simple messages)
- Circuit breaker integration (failure rate tracking per task)

## Configuration

New/extended config sections:

```toml
[agent]
# Existing (from liveness-detection.md)
stale_threshold = 10
wake_grace_period = 2
sleep_gap_threshold = 30
stale_tick_threshold = 2
retry_cooldown = 60

# New — lifecycle
session_persistence = true        # default: persist sessions for resume
max_resurrections_per_task = 5    # rate limit
resurrection_cooldown = 60        # seconds between resurrections

[checkpoint]
auto_interval_turns = 15
auto_interval_mins = 20
max_checkpoints = 5

[wait]
condition_poll_interval = 10      # seconds between condition checks
max_wait_duration = 86400         # seconds (24h) — auto-fail if exceeded
grace_after_wait = 30             # seconds before SIGTERM if agent doesn't exit
```

## Committee Participants

| Researcher | Focus Area | Key Contribution |
|-----------|------------|-----------------|
| A1 | Message-triggered resurrection | Found `--no-session-persistence` blocker; security model (whitelist + rate limit) |
| A2 | Resurrection implementation | Child-task vs reopen analysis; conditional approach based on downstream state |
| B1 | Long-running checkpointing | Hybrid auto+explicit design; checkpoint data structure; triage integration |
| B2 | Checkpointing systems analysis | Tiered approach; "orientation context" framing; handoff doc sufficiency |
| C1 | Unified lifecycle state machine | Blocked vs Waiting distinction; Stuck-as-field; formal state analysis |
| D1 | `wg wait` design + conditions | Park-and-exit semantics; condition DSL; unified resume pipeline insight |

## Dissenting Opinions

**Minor disagreement on Ready (stored vs computed):** B1 and Host favored stored-with-recomputation for large graph performance. D1 favored computed for simplicity. Consensus: defer to implementation — start computed, optimize to stored if profiling shows need.

**A2 evolved position on resurrection:** Initially proposed child-task-only, then revised to conditional (reopen when safe, child task when downstream running). A1 preferred always-reopen. Final consensus: A2's conditional approach adopted by all.

**No major dissent.** All researchers confirmed the synthesis positions.

## References

- Liveness detection (Round 1): `docs/design/liveness-detection.md`
- wg wait detailed design: `docs/design/wg-wait-design.md`
- Lifecycle state machine: `docs/design/unified-lifecycle-state-machine.md`
- Resurrection research: `docs/research/message-triggered-resurrection.md`
- Checkpointing analysis: `docs/research/checkpointing-systems-analysis.md`
- Existing triage: `src/commands/service/triage.rs`
- Coordinator main loop: `src/commands/service/mod.rs`
- Stream events: `src/stream_event.rs`
- Executor spawn: `src/commands/spawn/execution.rs`
- Config: `src/config.rs`
