# Design: Real-Time Sync & the 'Live Workspace' Feel

**Task:** mu-design-live-sync
**Date:** 2026-03-25
**Depends on:** mu-research (multi-user TUI feasibility)

---

## 1. Design Goals

The user's words: *"having everything somehow simultaneous, like live, it gives a level of activity, like a life to the system"*

This design addresses two coupled problems:

1. **Technical sync** — How changes propagate between users/views in near-real-time
2. **UX of liveness** — How the TUI conveys activity, presence, and momentum

And a third operational concern:

3. **Surveillance** — How a team of humans can oversee long-running agent processes

### Latency Targets

| Interaction | Target | Rationale |
|-------------|--------|-----------|
| Local graph mutation → other TUI instances | < 100ms | Feels instantaneous to humans |
| Agent spawns/completes → TUI update | < 200ms | Keeps dashboard current |
| User presence change → other users see it | < 2s | Presence is ambient, not urgent |
| Remote (git-based) sync | < 30s | Acceptable for async collaboration |
| Alert on failure/stuck agent | < 5s | Operational safety |

---

## 2. Sync Architecture

### 2.1 The Existing Foundation (What Already Works)

The current infrastructure is stronger than it appears:

| Component | Location | What It Does |
|-----------|----------|--------------|
| **fs watcher** | `state.rs:3857` | `notify_debouncer_mini` watches `.wg/` recursively, 50ms debounce |
| **mtime polling** | `state.rs:3887` | 1-second fallback when fs watcher unavailable |
| **flock serialization** | `parser.rs` | `modify_graph()` holds exclusive flock across load→modify→save |
| **atomic rename** | `parser.rs` | temp-file-rename ensures readers never see partial writes |
| **IPC GraphChanged** | `ipc.rs:54` | CLI commands notify daemon, triggering immediate coordinator tick |
| **screen dump IPC** | `screen_dump.rs` | Unix socket at `.wg/service/tui.sock`, per-frame snapshot |
| **provenance log** | `provenance.rs` | `operations.jsonl` records every mutation with timestamp, actor, detail |
| **notification router** | `notify/mod.rs` | Multi-channel dispatch (Telegram, Slack, Matrix, email, webhook) |
| **firehose panel** | `state.rs:1541` | Merged real-time stream of all agent output |

**For the single-VPS case (all users on same filesystem), the sync problem is already solved.** Each TUI instance has its own fs watcher; mutations via `modify_graph()` are atomic and flock-serialized; the firehose already shows agent activity in real-time.

### 2.2 Local Sync: Event Bus Enhancement

The fs watcher provides sub-100ms detection, but it's a blunt instrument — it fires on *any* file change without saying *what* changed. This limits the TUI to full-graph reloads.

**Proposed: Structured event bus via the daemon's IPC socket.**

The daemon already accepts `GraphChanged` IPC requests. Extend this into a **publish-subscribe event bus**:

```
┌──────────┐    ┌──────────┐    ┌──────────┐
│  TUI #1  │    │  TUI #2  │    │  Agent   │
│(user A)  │    │(user B)  │    │ process  │
└────┬─────┘    └────┬─────┘    └────┬─────┘
     │               │               │
     │  subscribe    │  subscribe    │  publish
     │               │               │
     ▼               ▼               ▼
┌─────────────────────────────────────────────┐
│          Service Daemon (event bus)          │
│                                             │
│  Event types:                               │
│  - GraphMutated { task_id, op, actor }      │
│  - AgentSpawned { agent_id, task_id }       │
│  - AgentCompleted { agent_id, task_id }     │
│  - AgentFailed { agent_id, task_id, reason }│
│  - LogAppended { task_id, line_count }      │
│  - PresenceChanged { user, action }         │
│  - CoordinatorTick { decisions }            │
│                                             │
│  Delivery: Unix socket, JSONL stream        │
│  Semantics: at-most-once, no persistence    │
└─────────────────────────────────────────────┘
```

**Why the daemon, not a separate process?** The daemon already manages the IPC socket, knows about all agents, and handles coordinator ticks. Adding pub-sub is a natural extension of `handle_connection()`.

**Protocol:**

```json
// Subscribe (new IPC request type)
{"cmd": "subscribe", "events": ["graph_mutated", "agent_spawned", "agent_completed"]}

// Events arrive as newline-delimited JSON on the same connection:
{"event": "graph_mutated", "task_id": "impl-auth", "op": "status_change", "actor": "agent-1234", "ts": "2026-03-25T20:00:00Z"}
{"event": "agent_spawned", "agent_id": "agent-5678", "task_id": "fix-bug", "ts": "2026-03-25T20:00:01Z"}
```

**TUI integration:** The TUI connects to the daemon's event bus on startup. Instead of the blunt fs-watcher-triggered full reload, it receives typed events and performs targeted updates:
- `GraphMutated` on a task → reload that task's node, update stats
- `AgentSpawned` → add to agent monitor, update HUD
- `LogAppended` → append to firehose if auto-tail is on

The fs watcher remains as fallback for when the daemon isn't running (standalone TUI mode).

**Implementation cost:** Medium. The daemon's `handle_connection()` currently processes one request per connection. Change to a persistent connection model for subscribers: spawn a thread per subscriber, write events to a `broadcast::Sender<Event>` (tokio broadcast channel or `std::sync::mpsc`).

### 2.3 Git-Based Distributed Sync

For non-colocated users (separate machines), the graph needs to sync over a network. The research identified three tiers; here is the concrete design for each.

#### Tier 1: Manual Git Sync (works today)

Users commit and push/pull `.wg/graph.jsonl` like any other file. JSONL format means conflicts are isolated to individual lines (tasks). This works for infrequent, non-overlapping changes.

**No code changes required.** Document this as a supported workflow.

#### Tier 2: Custom Git Merge Driver

Register a custom merge driver in `.gitattributes`:

```
.wg/graph.jsonl merge=wg-jsonl
```

The merge driver (`wg merge-driver`) resolves conflicts using domain knowledge:

| Field | Merge Strategy |
|-------|---------------|
| `status` | Lattice merge: `open < in-progress < done/failed` — take the "more advanced" status |
| `logs` | Union of log entries, deduplicated by timestamp+content hash |
| `title`, `description` | Last-writer-wins by timestamp (require timestamps on mutations) |
| `edges` | Union of edges (adding a dependency is always safe; removing requires tombstone) |
| `tags`, `skills` | Set union |
| New task (different IDs) | Accept both — no conflict |
| Same task ID, different content | Field-by-field merge using above rules |

**Implementation cost:** Medium. ~300-500 lines of Rust for the merge driver. Requires adding a `last_modified` timestamp to graph nodes (or using the provenance log).

**Latency:** Bounded by git push/pull frequency. A `wg sync` command could automate: `git add .wg/ && git commit -m "wg sync" && git pull --rebase && git push`.

#### Tier 3: Operation-Log CRDT (Future)

For real-time multi-machine sync, redesign storage from state snapshots to an operation log:

```
# Instead of storing task state:
{"id":"task-1","title":"Fix bug","status":"done","logs":[...]}

# Store operations:
{"op":"create","id":"task-1","title":"Fix bug","ts":"2026-03-25T20:00:00Z","clock":[1,0]}
{"op":"set_status","id":"task-1","status":"in-progress","ts":"...","clock":[2,0]}
{"op":"append_log","id":"task-1","text":"Started work","ts":"...","clock":[3,0]}
{"op":"set_status","id":"task-1","status":"done","ts":"...","clock":[4,0]}
```

Operations are commutative (can be applied in any order) using:
- **Vector clocks** for causal ordering
- **Status lattice** for conflict-free status merges
- **Set-union** for logs, tags, edges
- **LWW-register** for title, description

The current `operations.jsonl` provenance log (`src/provenance.rs`) is already close to this format. The gap is: it records *what happened* but isn't used as the *source of truth*. Promoting it to the primary storage format is the CRDT migration.

**Implementation cost:** High. Requires rethinking `load_graph`/`save_graph` to materialize state from an operation log, adding vector clocks, and building a replication protocol (TCP or WebSocket).

**Recommendation:** Defer to Phase 4. The single-VPS model (Phase 1-2) doesn't need it, and the git merge driver (Tier 2) handles occasional multi-machine sync.

### 2.4 Sync Architecture Summary

```
Phase 1 (now)        Phase 2 (soon)       Phase 3 (later)      Phase 4 (future)
─────────────        ──────────────       ───────────────      ───────────────
fs watcher +         Event bus via        Git merge driver     Operation-log
flock + atomic       daemon IPC           for graph.jsonl      CRDT replication
rename               (pub-sub)

Latency: <100ms      Latency: <50ms       Latency: git         Latency: <100ms
Scope: local         Scope: local         Scope: multi-machine  Scope: real-time
                                                                multi-machine
```

---

## 3. Liveness UX Design

### 3.1 Activity Feed

**Concept:** A chronological stream of system events, surfaced as a TUI panel. Unlike the firehose (raw agent output), the activity feed shows *semantic events* at the system level.

**Events to surface:**

| Event | Display | Source |
|-------|---------|--------|
| Task created | `+ task-id "Title"` | GraphMutated |
| Task status change | `task-id: open → in-progress` | GraphMutated |
| Agent spawned | `▶ agent-1234 → task-id` | AgentSpawned |
| Agent completed | `✓ agent-1234 (task-id) in 2m30s` | AgentCompleted |
| Agent failed | `✗ agent-1234 (task-id): reason` | AgentFailed |
| Coordinator decision | `⟳ coordinator: spawning 2 agents` | CoordinatorTick |
| User action | `user@erik: marked task-id done` | GraphMutated + WG_USER |
| Verification result | `⊘ task-id: verify failed (attempt 2/3)` | GraphMutated |

**TUI placement:** Replace or augment the existing "Coord Log" tab (panel 6) with an activity feed. The coord log currently shows raw daemon log lines — restructure into typed events.

**Implementation:**

The event bus (Section 2.2) provides the raw events. The TUI subscribes and formats them into human-readable activity lines. Each line gets:
- Timestamp (relative: "2s ago", "1m ago")
- Icon/color per event type
- Task ID as a clickable reference (jump-to-task on Enter)

**Scrollback:** Keep last 500 events in memory (ring buffer). Auto-tail by default; manual scroll disables auto-tail (same pattern as firehose).

### 3.2 Presence Indicators

**Concept:** Show which users are connected and what they're looking at, creating ambient awareness without requiring direct communication.

**Mechanism:**

Each TUI instance registers its presence via the daemon's IPC:

```json
{"cmd": "presence", "user": "erik", "view": "graph", "selected_task": "impl-auth"}
```

Sent on:
- TUI startup (join)
- Tab/task selection change (update)
- TUI exit (leave)
- Periodic heartbeat (every 30s, to detect stale sessions)

**Where to show:**

1. **HUD bar** (bottom of graph panel): `👤 erik(graph) 👤 alice(task:fix-bug)` — compact, always visible
2. **Task detail panel**: If another user is viewing the same task, show `Also viewing: alice`
3. **Graph visualization**: Subtle glow/highlight on tasks that other users are currently focused on

**Stale detection:** If no heartbeat in 60s, mark user as `idle`. After 5 minutes, remove from presence list.

**Privacy:** Presence is opt-in. `wg config --presence off` disables broadcasting. The daemon only stores presence for the current session (not persisted to disk).

### 3.3 Heartbeat / System Vitals

**Concept:** Even when no tasks are actively changing, the system should feel *alive* — not frozen or crashed. This is the difference between "nothing is happening" and "the system is healthy and watching."

**Always-visible vitals in the HUD:**

```
┌─────────────────────────────────────────────────────────────────────┐
│ ● 3 agents running │ 12 tasks open │ last event 4s ago │ ▲ 2 users │
└─────────────────────────────────────────────────────────────────────┘
```

| Indicator | Source | Update Frequency |
|-----------|--------|-----------------|
| Agent count (running/total) | Agent registry | Every coordinator tick (~10s) |
| Task counts by status | Graph stats | On graph change |
| Time since last event | Event bus | Every second (timer) |
| Connected users | Presence system | On presence change |
| Coordinator status | Coordinator state | Every tick |
| Service uptime | Daemon state | On status query |

**The "time since last event" counter is crucial.** It provides the heartbeat feel:
- `last event 2s ago` → system is actively working
- `last event 30s ago` → system is idle but healthy
- `last event 5m ago` → nothing happening (normal)
- `⚠ no coordinator` → system needs attention

**Visual rhythm:** When events arrive in bursts (multiple agents completing tasks), briefly flash/highlight the vitals bar. When the system goes from idle to active (first event after >30s quiet), pulse the status indicator.

### 3.4 Agent Activity Visualization

**Concept:** Running agents are the system's "workers." Making their activity visible creates the sense of a living, productive system.

**Current state:** The TUI already has:
- Agent monitor panel (shows running agents, PIDs, task IDs)
- Firehose panel (merged agent output stream)
- Output panel (per-agent output, switchable)

**Enhancements:**

1. **Agent progress in the graph view.** Tasks with running agents get a spinner/animation:
   ```
   [impl-auth ⟳ agent-1234 2m15s]  →  [fix-bug ⟳ agent-5678 45s]
   ```
   The elapsed time provides a sense of momentum. The spinner provides visual "life."

2. **Resource usage.** Show token consumption per agent in the agent monitor:
   ```
   agent-1234 │ impl-auth │ 2m15s │ 12.3k tokens │ active
   agent-5678 │ fix-bug   │ 45s   │ 3.1k tokens  │ active
   ```

3. **Activity sparkline.** A tiny sparkline in the HUD showing event rate over the last 5 minutes:
   ```
   activity: ▁▂▅▇█▇▅▃▂▁▁▂▃▅▇  (last 5m)
   ```
   This immediately communicates whether the system is ramping up, steady, or winding down.

### 3.5 Notifications & Toasts

**Concept:** When something important happens, surface it immediately — don't require the user to be watching the right panel.

**TUI toast notifications** (already partially implemented via `self.notification`):

| Event | Toast | Duration |
|-------|-------|----------|
| Task completed | `✓ impl-auth completed (2m30s)` | 5s |
| Task failed | `✗ fix-bug failed: test assertion` | Until dismissed |
| Agent stuck (no output >5m) | `⚠ agent-1234 may be stuck (no output 5m)` | Until dismissed |
| Verification passed | `✓ impl-auth passed verification` | 5s |
| Verification failed | `⊘ impl-auth failed verification (attempt 2/3)` | 10s |
| New message for you | `💬 Message on task-id from alice` | 10s |

**Multi-user notifications:** When the event bus is active, the daemon can route significant events to the notification router (existing `notify/mod.rs`). A user on mobile (not looking at TUI) gets a Telegram/Slack notification for failures and stuck agents.

**Notification filtering:** Each user configures their interest level:
```toml
# .wg/config.toml
[notify.tui]
show = ["task_failed", "verification_failed", "agent_stuck", "message_received"]
suppress = ["task_completed"]  # too noisy for large graphs
```

---

## 4. Surveillance Dashboard Design

### 4.1 Dashboard View: High-Level Status

**Concept:** A birds-eye view of the entire system, optimized for a team lead or operator who wants to know "is everything OK?" at a glance.

**Layout (full-screen TUI mode):**

```
┌──────────────────────────────────────────────────────────────────┐
│ WG DASHBOARD                           ▲ 3 users │ uptime 4h22m │
├──────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Coordinators                                                    │
│  ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐   │
│  │ coord-0         │ │ coord-1         │ │ coord-2         │   │
│  │ ● Running       │ │ ● Running       │ │ ○ Idle          │   │
│  │ 2 agents active │ │ 1 agent active  │ │ 0 agents        │   │
│  │ last tick 3s    │ │ last tick 8s    │ │ last tick 45s   │   │
│  │ 15 tasks managed│ │ 8 tasks managed │ │ 3 tasks managed │   │
│  └─────────────────┘ └─────────────────┘ └─────────────────┘   │
│                                                                  │
│  Agents                                                          │
│  agent-1234 │ coord-0 │ impl-auth     │ 2m15s │ ● active       │
│  agent-5678 │ coord-0 │ fix-bug       │ 45s   │ ● active       │
│  agent-9012 │ coord-1 │ add-tests     │ 8m30s │ ⚠ slow         │
│                                                                  │
│  Recent Events                                                   │
│  20:04:38 ✓ agent-3456 completed review-pr (coord-1)            │
│  20:04:24 ▶ agent-9012 spawned for add-tests (coord-1)          │
│  20:03:55 ✓ agent-7890 completed fix-typo (coord-0)             │
│  20:03:12 ⟳ coord-0 tick: spawned agent-5678 for fix-bug        │
│                                                                  │
│  Graph Summary                                                   │
│  open: 8  │  in-progress: 3  │  done: 45  │  failed: 1         │
│  activity: ▁▂▅▇█▇▅▃▂▁▁▂▃▅▇  (last 30m)                       │
│                                                                  │
├──────────────────────────────────────────────────────────────────┤
│ [d]rill agent │ [t]ask detail │ [l]ogs │ [a]lerts │ [q]uit      │
└──────────────────────────────────────────────────────────────────┘
```

**Implementation:** This is a new TUI mode (`wg tui --dashboard` or a new tab in the existing TUI). It composes existing data sources:
- Coordinator state from `coordinator-state.json`
- Agent list from agent registry
- Events from the event bus
- Graph stats from `load_graph()`

### 4.2 Drill-Down: Agent → Task → Logs

From the dashboard, the user can drill into any agent:

```
Dashboard → select agent-9012
  → Agent Detail:
    - Task: add-tests
    - Started: 20:04:24 (8m30s ago)
    - Token usage: 45.2k
    - Output: [streaming, last line 15s ago]
    - Status: ⚠ slow (no output in 30s, threshold 30s)

    [v]iew output │ [t]ask detail │ [k]ill agent │ [b]ack

    → view output: Shows the Output panel for this specific agent
    → task detail: Shows the task's full detail (description, logs, deps)
```

This is the **drill-down chain**: Dashboard → Agent → Task → Logs. Each level provides more detail, and the user can jump back at any point.

**Implementation:** All these views already exist in the TUI as separate panels. The drill-down is navigation logic: selecting an agent in the dashboard switches to the Output tab filtered to that agent, then to the Detail tab for its task.

### 4.3 Alerts & Configurable Triggers

**Alert conditions:**

| Condition | Detection | Default Threshold | Severity |
|-----------|-----------|-------------------|----------|
| Agent stuck (no output) | Compare last output timestamp | 5 minutes | Warning |
| Agent overtime | Compare elapsed vs. task model tier timeout | 30m (haiku), 1h (sonnet), 2h (opus) | Warning |
| Task failed | Status change to `failed` | Immediate | Error |
| Verification failed (final attempt) | Max retries exhausted | Immediate | Error |
| Coordinator offline | No heartbeat from daemon | 60s | Critical |
| Graph write contention | Flock wait exceeds threshold | 5s | Warning |
| Disk space low | Check filesystem | < 1GB | Critical |

**Alert routing:**

```toml
# .wg/config.toml
[alerts]
# TUI toasts for everything
tui = ["warning", "error", "critical"]
# Telegram only for errors and critical
telegram = ["error", "critical"]
# Email digest for warnings (batched every 15m)
email = { levels = ["warning"], batch_interval = 900 }
```

Alerts use the existing notification router (`notify/mod.rs`). The daemon monitors alert conditions on each coordinator tick and dispatches through configured channels.

**Alert deduplication:** Same condition doesn't fire twice within a cooldown period (default 5 minutes per alert type per subject). Prevents notification storms.

---

## 5. Scalability Analysis

### 5.1 Concurrent Users: How Many Before Degradation?

The bottleneck chain for the single-VPS model:

```
Write path:  modify_graph() → flock(EX) → read → modify → write → rename → unlock
Read path:   load_graph() → flock(SH|NB) → read → parse
Notification: inotify event → debounce (50ms) → TUI reload
```

**Modeled performance:**

| Concurrent Users | Write Throughput | Read Latency | Notification Latency | Verdict |
|-----------------|-----------------|--------------|---------------------|---------|
| 1-2 | 20-50 writes/sec | 5-20ms | <100ms | No issues |
| 3-5 | 15-40 writes/sec | 5-20ms | <100ms | Minimal contention |
| 5-10 | 10-30 writes/sec | 10-50ms | <200ms | Noticeable write queuing |
| 10-20 | 5-15 writes/sec | 20-100ms | <500ms | Write contention is real |
| 20+ | <5 writes/sec | 100ms+ | <1s | Degraded; consider sharding |

**Key insight:** Read operations are non-blocking (shared flock). Only *writes* are serialized. In practice, most users are reading (watching the dashboard, browsing tasks) most of the time. A 10-user team with 2-3 concurrent writers is well within comfort.

**What degrades first:**
1. Write latency (flock contention) — users feel delay when marking tasks done
2. inotify notification storm — many watchers on same directory create kernel overhead
3. Graph file size — 10k+ tasks with full log history → slow full reloads

### 5.2 Mitigation Strategies

| Problem | Threshold | Mitigation |
|---------|-----------|-----------|
| Write contention | >10 concurrent writers | Batch mutations: daemon accepts mutation queue, applies in batch under single flock |
| inotify storm | >20 watchers | Event bus (Section 2.2) replaces per-instance fs watching with single daemon watcher |
| Graph file size | >5MB | Graph compaction (already exists as `.compact-0`): archive completed tasks to separate file |
| Large graph reload | >500ms parse time | Incremental reload: event bus tells TUI which tasks changed, update in-memory graph without full re-parse |

### 5.3 Event Bus as Scalability Lever

The event bus (Section 2.2) is the key scalability improvement. Instead of N TUI instances each watching the filesystem independently:

```
Without event bus:                 With event bus:
N watchers × M files = N×M        1 watcher (daemon) + N subscribers
inotify events                     = N event deliveries (typed, targeted)
```

For 10 users, this reduces inotify load by 10x and replaces full-graph reloads with targeted updates.

---

## 6. Implementation Roadmap

### Phase 1: Liveness Polish (no architectural changes)

Estimated scope: ~500-800 lines of Rust

1. **Activity sparkline in HUD** — Compute event rate from provenance log, render as sparkline widget
2. **"Time since last event" counter** — Read last entry from `operations.jsonl`, display in HUD
3. **Agent elapsed time in graph view** — Already have agent→task mapping; add duration to node rendering
4. **Enhanced toast notifications** — Extend existing `self.notification` with multi-level severity
5. **Dashboard tab** — New `RightPanelTab::Dashboard` composing existing stats, agent monitor, and coordinator state

### Phase 2: Event Bus + Presence

Estimated scope: ~1500-2000 lines of Rust

1. **IPC subscribe command** — Extend `IpcRequest` with `Subscribe { events: Vec<String> }`
2. **Event broadcast in daemon** — Spawn thread per subscriber, broadcast channel for events
3. **TUI event bus client** — Connect to daemon on startup, receive typed events
4. **Targeted refresh** — Replace blunt fs-watcher reloads with event-driven updates for subscribed TUIs
5. **Presence protocol** — IPC presence command, daemon tracks connected users, TUI displays in HUD
6. **Activity feed panel** — New panel showing formatted event stream (replacing raw coord log)

### Phase 3: Git Sync + Alerts

Estimated scope: ~1000-1500 lines of Rust

1. **`wg merge-driver`** — Custom git merge driver for `graph.jsonl`
2. **`wg sync`** — Convenience command for commit + pull + push of graph state
3. **Alert monitoring** — Daemon checks alert conditions on each tick
4. **Alert routing** — Connect to notification router with configurable thresholds
5. **Alert deduplication** — Cooldown tracking per condition/subject

### Phase 4: CRDT (if needed)

Only if multi-machine real-time sync becomes a real requirement. The operation-log format is sketched in Section 2.3 Tier 3.

---

## 7. Key Architectural Decisions

### Decision 1: Event Bus Location

**Chosen: In the daemon process.**

The daemon already manages IPC, agents, and coordinator ticks. It's the natural event aggregation point. Adding pub-sub to the daemon avoids a new process and reuses the existing Unix socket infrastructure.

Alternative considered: Separate event bus process. Rejected — adds operational complexity for no architectural benefit.

### Decision 2: Presence Storage

**Chosen: In-memory only, daemon process.**

Presence is ephemeral by nature. Persisting it to disk would create stale data problems. The daemon holds a `HashMap<String, PresenceInfo>` with heartbeat expiry.

### Decision 3: Activity Feed vs. Enhanced Firehose

**Chosen: Separate activity feed panel.**

The firehose shows raw agent output (stdout). The activity feed shows system-level events (task created, agent spawned, status changes). These serve different audiences:
- **Firehose:** Developer debugging a specific agent's behavior
- **Activity feed:** Team lead watching system progress

Merging them would make both less useful.

### Decision 4: Graph View Animation

**Chosen: Spinner + elapsed time on in-progress tasks.**

Considered: Per-task progress bars, animated edges, pulsing nodes. Rejected as too visually noisy. A simple spinner (cycling `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`) plus elapsed time is informative without being distracting.

### Decision 5: Alert Severity Model

**Chosen: Three levels — Warning, Error, Critical.**

- **Warning:** Something unusual but not blocking (agent slow, write contention)
- **Error:** Something failed but system continues (task failed, verification failed)
- **Critical:** System health issue requiring attention (coordinator offline, disk full)

Each level maps to a different notification urgency, controlling which channels fire.

---

## 8. UX Mockups

### 8.1 Activity Feed Panel

```
┌─ Activity ──────────────────────────────────────────┐
│                                                      │
│  20:04:38  ✓  review-pr completed (agent-3456, 3m)  │
│  20:04:24  ▶  agent-9012 → add-tests                │
│  20:03:55  ✓  fix-typo completed (agent-7890, 1m)   │
│  20:03:12  ⟳  coordinator tick: spawned 2 agents     │
│  20:02:45  +  new task: add-tests                    │
│  20:02:30  ↗  impl-auth: open → in-progress          │
│  20:02:30  ▶  agent-1234 → impl-auth                 │
│  20:01:15  👤 alice connected                         │
│  20:00:00  ⟳  coordinator tick: 0 agents spawned     │
│                                                      │
│  ▼ auto-tail                                         │
└──────────────────────────────────────────────────────┘
```

### 8.2 HUD Bar with Vitals + Presence

```
┌──────────────────────────────────────────────────────────────────────┐
│ ● 3 agents │ 8/56 open │ last: 4s │ ▁▃▅▇█▅▂ │ 👤erik 👤alice(fix-bug) │
└──────────────────────────────────────────────────────────────────────┘
```

- `● 3 agents` — green dot, count of running agents
- `8/56 open` — open tasks / total tasks
- `last: 4s` — time since last event (green <30s, yellow 30s-5m, red >5m)
- `▁▃▅▇█▅▂` — activity sparkline (last 10 minutes)
- `👤erik 👤alice(fix-bug)` — connected users with current focus

### 8.3 Toast Notification Stack

```
                              ┌─────────────────────────────────────┐
                              │ ✓ impl-auth completed (2m30s)      │ ← fades after 5s
                              ├─────────────────────────────────────┤
                              │ ⚠ agent-9012 may be stuck (5m)     │ ← persists until dismissed
                              └─────────────────────────────────────┘
```

Toasts stack from the bottom-right corner of the terminal. Multiple toasts are visible simultaneously (max 3). Older toasts slide up as new ones arrive.

### 8.4 Graph View with Agent Activity

```
                    ┌──────────────────┐
                    │ mu-research      │
                    │ ✓ done (3m)      │
                    └────────┬─────────┘
                             │
              ┌──────────────┼──────────────┐
              │              │              │
   ┌──────────▼───┐  ┌──────▼──────┐  ┌───▼───────────┐
   │ mu-design-   │  │ mu-design-  │  │ mu-design-    │
   │ live-sync    │  │ access      │  │ identity      │
   │ ⟳ 2m15s      │  │ ○ open      │  │ ⟳ 45s         │
   └──────────────┘  └─────────────┘  └───────────────┘
```

Tasks with running agents show `⟳` spinner + elapsed time. Completed tasks show `✓` + duration. Open tasks show `○`.

---

## 9. References

| Resource | Location |
|----------|----------|
| Multi-user research | `docs/research/multi-user-tui-feasibility.md` |
| File locking audit | `docs/research/file-locking-audit.md` |
| Cross-repo communication | `docs/design/cross-repo-communication.md` |
| IPC protocol | `src/commands/service/ipc.rs` |
| Screen dump IPC | `src/tui/viz_viewer/screen_dump.rs` |
| Notification router | `src/notify/mod.rs` |
| Provenance system | `src/provenance.rs` |
| Firehose panel | `src/tui/viz_viewer/state.rs:1525-1574` |
| fs watcher | `src/tui/viz_viewer/state.rs:3855-3883` |
| Coordinator state | `src/commands/service/mod.rs:310-400` |
| Liveness detection | `docs/design/liveness-detection.md` |
