# Design: Coordinator as a Visible Entity in the Task Graph

## Status

Proposed (2026-03-05)

## Problem

The coordinator daemon is invisible in the graph. It spawns agents, assigns tasks,
creates `.assign-*` and `.evaluate-*` system tasks, manages cycles, triages dead
agents, and processes chat messages -- but none of this is surfaced as a first-class
entity. Users can only observe coordinator behavior through:

- `wg service status` / `wg status` (running/stopped, last tick, PID)
- `wg service log` (daemon.log file)
- Inferring from `.assign-*` / `.evaluate-*` task presence

This makes it hard to answer everyday questions: Is the coordinator doing anything?
Why hasn't my task been picked up? When did it last compact/evaluate? Is it stuck?

## Design Questions & Answers

### 1. Should the coordinator be a persistent task node?

**No.** A task implies work-to-be-done with a lifecycle (open -> in-progress -> done).
The coordinator is an ongoing process, not a unit of work. Making it a task would
pollute the graph with a permanently in-progress node that never completes, and it
would appear in `wg list`, `wg viz`, task counts, and ready-task queries where it
doesn't belong.

### 2. Or a special entity type visible in the graph/TUI?

**Yes -- a new `ServiceEntity` stored outside the graph but rendered in the TUI.**

The coordinator is best modeled as a *sidecar entity* -- not a graph node, but a
first-class object with its own state file that the TUI and CLI can query. This
follows the same pattern as `AgentRegistry` (`.wg/service/agents.json`):
operational metadata stored alongside the graph, not in it.

### 3. How would coordinator activities show up?

**As a structured activity log on the coordinator entity**, not as child tasks.

Current state: The daemon already writes `daemon.log` as unstructured text.
The proposal adds a structured `coordinator-activity.jsonl` file that records
typed events:

| Event type | Fields | When emitted |
|---|---|---|
| `tick` | tick_number, agents_alive, tasks_ready, agents_spawned, duration_ms | Every coordinator tick |
| `assign` | task_id, agent_hash, assign_task_id | Task assignment completed |
| `evaluate` | task_id, eval_task_id, verdict | Evaluation completed |
| `triage` | agent_id, task_id, verdict, reason | Dead agent triaged |
| `cycle_iterate` | cycle_header, tasks_reactivated | Cycle reactivated |
| `cycle_failure_restart` | cycle_header, tasks_reactivated | Cycle restarted after failure |
| `wait_resume` | task_id, condition_type | Waiting task resumed |
| `resurrect` | task_id, reason | Done task reopened by message |
| `spawn` | agent_id, task_id, executor, model | Agent spawned |
| `error` | phase, message | Any tick phase error |
| `chat` | request_id, routed_to | Chat message routed |
| `pause` / `resume` | - | Coordinator paused/resumed |

This replaces the unstructured `[coordinator] ...` eprintln messages with
machine-readable events while keeping daemon.log for low-level debugging.

### 4. What about multiple coordinators or restarts?

The coordinator entity is **singleton per wg** and persists across restarts:

- `coordinator-state.json` already persists tick count, config, last tick time
- The new `coordinator-activity.jsonl` appends across sessions (rotated by size)
- On startup, a `start` event is appended; on shutdown, a `stop` event
- If the daemon crashes without writing `stop`, the next startup detects this
  (stale PID in service state) and records a `crash_detected` event

Multiple coordinators are not supported today (service start rejects if already
running). If federation adds multi-coordinator support in the future, the entity
model would extend to per-coordinator instances keyed by PID or UUID.

### 5. How does this interact with `wg service status`?

`wg service status` (and `wg status`) already reads `CoordinatorState`. The
proposal enriches this with recent activity:

```
Service: running (PID 12345, 2h 15m uptime)
Coordinator: max=4, executor=claude, model=default, poll=60s
  Last tick: 30s ago (#142) — 3 alive, 2 ready, 1 spawned
  Recent activity:
    14:32  assign   design-coordinator-as → agent-abc123
    14:31  spawn    agent-abc123 on design-coordinator-as
    14:30  triage   agent-dead1 on fix-bug-x (verdict: fail)
    14:28  evaluate impl-feature-y → pass (score: 0.92)
```

The `CoordinatorState` struct gets a new `recent_activity: Vec<ActivityEntry>`
field (last N events) so `wg status` can display them without parsing the full
activity log.

### 6. Should the TUI show coordinator status in a header/footer or as a graph node?

**Both, layered by context:**

**A. Status bar indicator (always visible)**

The top status bar already shows task counts. Add a coordinator heartbeat indicator
at the far right:

```
 42 tasks (28 done, 8 open, 4 active, 2 failed) | ... | C: 3/4 agents ●
```

Where `●` is green (healthy), yellow (paused), red (error on last tick), or gray
(stopped). The `3/4` shows alive/max agents.

**B. Agency tab enrichment (right panel)**

The existing Agency tab in the TUI shows agent entries. Add a "Coordinator" section
at the top:

```
── Coordinator ──────────────────────────────
  Status: running (tick #142, 30s ago)
  Agents: 3/4 (1 slot available)
  Paused: no

  Recent:
    14:32  assigned design-coordinator-as
    14:31  spawned agent-abc123
    14:30  triaged agent-dead1 → fail
    14:28  evaluated impl-feature-y → pass

── Agents ───────────────────────────────────
  agent-abc123  design-coordinator-as  12m  working
  agent-def456  impl-feature-y         5m  working
  ...
```

**C. NOT as a graph node** -- the coordinator is infrastructure, not a task.
Adding it to the graph visualization would break layout algorithms, add visual
noise, and confuse the mental model (tasks depend on tasks, not on infrastructure).

## Proposed Data Model Changes

### New: `CoordinatorActivity` event types

```rust
// src/service/activity.rs (new file)

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub timestamp: String,     // ISO 8601
    pub tick: Option<u64>,     // Which tick this occurred in
    pub event: ActivityEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActivityEvent {
    Start { pid: u32 },
    Stop { reason: String },
    CrashDetected { stale_pid: u32 },
    Tick { agents_alive: usize, tasks_ready: usize, agents_spawned: usize, duration_ms: u64 },
    Assign { task_id: String, agent_hash: Option<String>, assign_task_id: String },
    Evaluate { task_id: String, eval_task_id: String, verdict: Option<String> },
    Triage { agent_id: String, task_id: String, verdict: String, reason: String },
    Spawn { agent_id: String, task_id: String, executor: String, model: Option<String> },
    CycleIterate { cycle_header: String, tasks_reactivated: Vec<String> },
    CycleFailureRestart { cycle_header: String, tasks_reactivated: Vec<String> },
    WaitResume { task_id: String, condition_type: String },
    Resurrect { task_id: String, reason: String },
    Chat { request_id: String },
    Pause,
    Resume,
    Error { phase: String, message: String },
}
```

### Storage

File: `.wg/service/coordinator-activity.jsonl`

- Append-only JSONL (one `ActivityEntry` per line)
- Rotated when > 1MB (keep one backup: `coordinator-activity.jsonl.1`)
- Read by TUI and `wg status` for recent activity display

### Extended: `CoordinatorState`

```rust
// In src/commands/service/mod.rs, extend existing struct:

pub struct CoordinatorState {
    // ... existing fields ...

    /// Last N activity events (ring buffer, persisted for quick status queries)
    #[serde(default)]
    pub recent_activity: Vec<ActivityEntry>,

    /// Timestamp of daemon start (to detect crashes across restarts)
    #[serde(default)]
    pub started_at: Option<String>,

    /// Whether the last tick had errors
    #[serde(default)]
    pub last_tick_error: Option<String>,
}
```

## TUI Rendering Approach

### Status bar (always visible)

In `src/tui/viz_viewer/render.rs::draw_status_bar`:

```rust
// After task counts, add coordinator indicator
if let Some(coord) = &app.coordinator_state {
    let indicator = match (coord.enabled, coord.paused, coord.last_tick_error.is_some()) {
        (false, _, _) => ("C: off", Color::DarkGray),
        (_, true, _) => ("C: paused", Color::Yellow),
        (_, _, true) => ("C: error", Color::Red),
        _ => ("C: ok", Color::Green),
    };
    spans.push(Span::styled(
        format!("| {} {}/{} ", indicator.0, coord.agents_alive, coord.max_agents),
        Style::default().fg(indicator.1),
    ));
}
```

### Agency tab enrichment

In `src/tui/viz_viewer/render.rs`, the Agency tab render function adds a
coordinator section above the agent list. The section reads from
`app.coordinator_state` and `app.coordinator_recent_activity` (loaded from
the activity log on refresh ticks).

### VizApp state additions

```rust
// In src/tui/viz_viewer/state.rs, add to VizApp:
pub coordinator_state: Option<CoordinatorState>,
pub coordinator_recent_activity: Vec<ActivityEntry>,
```

Loaded in `refresh()` by reading `coordinator-state.json` and the last ~20
lines of `coordinator-activity.jsonl`.

## How Activities Get Logged

### Emit points in coordinator.rs

Each phase of `coordinator_tick` emits activity events:

| Phase | Code location | Events emitted |
|---|---|---|
| Phase 1: cleanup dead agents | `coordinator.rs:42` `cleanup_and_count_alive` | `Triage` for each dead agent |
| Phase 2.5: cycle iteration | `coordinator.rs:2021` | `CycleIterate` |
| Phase 2.6: cycle failure restart | `coordinator.rs:2037` | `CycleFailureRestart` |
| Phase 2.7: wait evaluation | `coordinator.rs:2054` | `WaitResume` for each woken task |
| Phase 2.8: resurrection | `coordinator.rs:2063` | `Resurrect` for each reopened task |
| Phase 3: auto-assign | `coordinator.rs:2074` | `Assign` per assignment |
| Phase 4: auto-evaluate | `coordinator.rs:2079` | `Evaluate` per evaluation created |
| Phase 6: spawn agents | `coordinator.rs:2100` | `Spawn` per agent |
| Tick wrapper | `mod.rs:1361` | `Tick` summary + `Error` on failure |

### Implementation pattern

The activity logger is passed into `coordinator_tick` as a parameter:

```rust
pub fn coordinator_tick(
    dir: &Path,
    max_agents: usize,
    executor: &str,
    model: Option<&str>,
    activity: &mut ActivityLog,  // NEW parameter
) -> Result<TickResult> { ... }
```

`ActivityLog` wraps the JSONL file handle and provides typed `record_*` methods:

```rust
impl ActivityLog {
    pub fn record(&mut self, tick: u64, event: ActivityEvent) { ... }
    pub fn recent(&self, n: usize) -> Vec<ActivityEntry> { ... }
}
```

### Daemon loop integration

In `run_daemon` (`mod.rs:1074`), the activity log is created at startup and
passed to each tick:

```rust
let mut activity_log = ActivityLog::open(dir)?;
activity_log.record(0, ActivityEvent::Start { pid: std::process::id() });

// In the tick section:
match coordinator::coordinator_tick(dir, max_agents, executor, model, &mut activity_log) {
    Ok(result) => {
        activity_log.record(coord_state.ticks, ActivityEvent::Tick { ... });
        // Update coord_state.recent_activity from activity_log.recent(10)
    }
    Err(e) => {
        activity_log.record(coord_state.ticks, ActivityEvent::Error { ... });
    }
}
```

## Migration / Backwards Compatibility

### Zero migration needed

- The activity log is a new file; its absence means "no activity recorded yet"
- `CoordinatorState` uses `#[serde(default)]` for new fields, so old state files
  deserialize without error
- The TUI gracefully handles `coordinator_state: None` (shows nothing)
- `wg status` shows the new "Recent activity" section only when data exists

### Deprecation path for daemon.log

- `daemon.log` continues to be written for the foreseeable future
- The activity log supplements it with structured data; it does not replace it
- Eventually, the unstructured `[coordinator] ...` eprintln messages could be
  reduced to `eprintln!` for error-only output, with all operational info going
  through the activity log

### Version detection

The `CoordinatorState` gains no version field. The presence/absence of the
`recent_activity` field (via `#[serde(default)]`) is sufficient. Old daemons
produce state without it; new daemons populate it. Readers handle both.

## Rough Implementation Plan

### Phase 1: Activity log infrastructure (small, foundational)

1. **New module `src/service/activity.rs`**: Define `ActivityEntry`, `ActivityEvent`,
   `ActivityLog` (open, record, recent, rotate).
2. **Wire into daemon loop** (`src/commands/service/mod.rs:1074`): Create `ActivityLog`
   on startup, pass to tick, record `Start`/`Stop`/`Tick` events.
3. **Extend `CoordinatorState`** (`src/commands/service/mod.rs:251`): Add
   `recent_activity`, `started_at`, `last_tick_error` with serde defaults.
4. **Populate `recent_activity`** from the last N events after each tick.

### Phase 2: Coordinator events in tick phases

5. **Thread `ActivityLog`** through `coordinator_tick` (`src/commands/service/coordinator.rs:1986`).
6. **Emit events** at each phase:
   - `Spawn` in `spawn_agents_for_ready_tasks` (`coordinator.rs:1700+`)
   - `Assign` in `build_auto_assign_tasks` (`coordinator.rs:790+`)
   - `Evaluate` in `build_auto_evaluate_tasks` (`coordinator.rs:1120+`)
   - `Triage` in `triage::cleanup_dead_agents` (`triage.rs:1+`)
   - `CycleIterate` / `CycleFailureRestart` in phases 2.5/2.6
   - `WaitResume` in phase 2.7 (`evaluate_waiting_tasks`)
   - `Resurrect` in phase 2.8 (`resurrect_done_tasks`)

### Phase 3: CLI integration

7. **Enrich `wg status`** (`src/commands/status.rs:38`): Read `recent_activity` from
   `CoordinatorState`, display last 5 events.
8. **New command `wg service activity`**: Tail/query the activity log with filters
   (by event type, by task ID, last N entries).

### Phase 4: TUI integration

9. **Add `coordinator_state` to `VizApp`** (`src/tui/viz_viewer/state.rs`): Load on
   init and refresh ticks.
10. **Status bar indicator** (`src/tui/viz_viewer/render.rs:3934`): Add coordinator
    health dot and agent count to the right side of the status bar.
11. **Agency tab coordinator section** (`src/tui/viz_viewer/render.rs`): Render
    coordinator status + recent activity above the agent list.

### Files to modify

| File | Change |
|---|---|
| `src/service/mod.rs` | Add `pub mod activity;` |
| `src/service/activity.rs` | **New**: ActivityEntry, ActivityEvent, ActivityLog |
| `src/commands/service/mod.rs` | Extend CoordinatorState, create ActivityLog in daemon loop |
| `src/commands/service/coordinator.rs` | Accept ActivityLog param, emit events in each phase |
| `src/commands/service/triage.rs` | Return structured triage results for activity logging |
| `src/commands/status.rs` | Display recent_activity in status output |
| `src/tui/viz_viewer/state.rs` | Add coordinator_state, load on refresh |
| `src/tui/viz_viewer/render.rs` | Status bar indicator, Agency tab section |
| `src/cli.rs` | Add `service activity` subcommand |

## Alternatives Considered

### A. Coordinator as a task node

Rejected. A permanently in-progress task pollutes counts, breaks `wg viz` layout,
and conflates infrastructure with work. The `.assign-*` / `.evaluate-*` tasks are
already the coordinator's "work products" in the graph -- the coordinator itself
is the producer, not a product.

### B. Coordinator activities as ephemeral child tasks

Rejected. Creating a task for every tick/assign/evaluate would bloat the graph
with hundreds of micro-tasks. The GC would need to clean them up, and they'd
appear in `wg list` unless filtered. The activity log is a better fit:
append-only, rotatable, and read-only from the graph's perspective.

### C. Extend daemon.log with structured events

Partially adopted. The activity log is essentially "structured daemon.log" for
coordinator-level events. Raw daemon.log remains for low-level debugging
(socket errors, panics, etc.).
