# Work Plan: TUI Liveness & Monitoring UX

**Task:** mu-plan-liveness
**Date:** 2026-03-25
**Depends on:** mu-design-synthesis (unified architecture), mu-design-live-sync (liveness UX design)
**Coordinates with:** mu-plan-server (server-side infrastructure)

---

## Overview

This plan breaks the liveness UX and monitoring features into 8 discrete implementation tasks, ordered by dependency and priority. The features transform the TUI from a passive viewer into an active, living dashboard that conveys system health, user presence, and agent activity in real time.

**Key architectural dependency:** Most liveness features have two modes — a **Phase 1 mode** (works without event bus, using fs watcher + polling existing state files) and a **Phase 2 mode** (event-driven via daemon IPC pub-sub). This plan designs all TUI features to work in Phase 1, then upgrade seamlessly when the event bus lands.

---

## Event Infrastructure Requirements

All liveness features depend on data flowing from the daemon to TUI instances. The infrastructure comes in two phases:

### Phase 1: Polling + fs watcher (no server changes needed)

| Data Source | Location | How TUI Reads It |
|-------------|----------|-------------------|
| Task counts / graph stats | `graph.jsonl` | `load_graph()` on fs watcher trigger (already works) |
| Agent list + status | `.wg/service/registry.json` | Poll on timer (already done in `reload_agents()`) |
| Coordinator state | `coordinator-state.json` | Poll on timer (already done in `reload_coordinator_state()`) |
| Provenance log (events) | `operations.jsonl` | Tail file on fs watcher trigger (new) |
| Daemon log (coord ticks) | `daemon.log` | Already read for CoordLog tab |

**Phase 1 delivers ~80% of the liveness UX with zero server changes.** The TUI tails `operations.jsonl` for the activity feed, polls the agent registry for dashboard data, and computes vitals from what it already loads.

### Phase 2: Event bus (requires mu-plan-server work)

| Requirement | Server-Side Task | What It Enables |
|-------------|-----------------|-----------------|
| `Subscribe` IPC command | mu-plan-server: event-bus | Persistent connection, typed event stream |
| `GraphMutated` events | mu-plan-server: event-bus | Targeted refresh (no full reload) |
| `AgentSpawned/Completed/Failed` events | mu-plan-server: event-bus | Real-time agent status in dashboard |
| `PresenceChanged` events | mu-plan-server: presence-protocol | User presence indicators |
| `Presence` IPC command | mu-plan-server: presence-protocol | TUI registers/updates its presence |
| Heartbeat acceptance | mu-plan-server: presence-protocol | Stale session detection |

### Dependency Map: mu-plan-server ↔ mu-plan-liveness

```
mu-plan-server tasks          mu-plan-liveness tasks
─────────────────────         ──────────────────────
                              
(no dependency)          ───→ Task 1: HUD Vitals Bar
(no dependency)          ───→ Task 2: Activity Feed (Phase 1)
(no dependency)          ───→ Task 3: Agent Dashboard Tab
(no dependency)          ───→ Task 4: Enhanced Toast Notifications
(no dependency)          ───→ Task 5: Drill-Down Navigation
event-bus                ───→ Task 6: Event Bus TUI Client
presence-protocol        ───→ Task 7: Presence Indicators
event-bus + presence     ───→ Task 8: Surveillance View (full)
```

**Tasks 1–5 are MVP and have NO dependency on mu-plan-server.** Tasks 6–8 require server infrastructure and are Phase 2.

---

## Task 1: HUD Vitals Bar

**Priority:** MVP (P1)
**Complexity:** Small
**TUI Component:** New widget in bottom status bar area (`render.rs`, `state.rs`)
**Server Dependency:** None (Phase 1: reads existing state)

### Description

Add an always-visible vitals strip to the TUI showing system health at a glance. This is the single most impactful liveness feature — it makes the difference between "is the system frozen?" and "the system is alive and working."

### Wireframe

```
┌─────────────────────────────────────────────────────────────────────────────┐
│ [Graph View]                              │ [Right Panel: Chat/Detail/...]  │
│                                           │                                 │
│   ┌─task-a──┐    ┌─task-b──┐              │                                 │
│   │ ● done  │───→│ ⟳ agent │              │                                 │
│   └─────────┘    └─────────┘              │                                 │
│                                           │                                 │
├───────────────────────────────────────────┴─────────────────────────────────┤
│ ● 2 agents │ 8 open · 3 running · 45 done │ last event 4s ago │ coord ● 3s │
└─────────────────────────────────────────────────────────────────────────────┘
```

**Vitals indicators:**

| Indicator | Source | Update Trigger |
|-----------|--------|----------------|
| Agent count (running) | `AgentRegistry` | Timer poll (already loaded) |
| Task status counts | `Graph` stats | fs watcher reload (already computed) |
| Time since last event | `operations.jsonl` mtime or last entry timestamp | 1-second timer tick |
| Coordinator heartbeat | `coordinator-state.json` last tick time | Timer poll |
| Activity sparkline (optional, Phase 1.5) | Event rate from provenance log | Computed on reload |

**"Time since last event" logic:**
- Read mtime of `operations.jsonl` (cheap syscall, no file parse needed)
- Display relative: `2s ago`, `30s ago`, `5m ago`
- Color coding: green (<30s), yellow (30s–5m), red (>5m), or `⚠ no coordinator` if daemon not running

### Files Modified

- `src/tui/viz_viewer/state.rs` — Add `last_event_time: Option<SystemTime>`, `vitals_visible: bool` fields to `VizApp`
- `src/tui/viz_viewer/render.rs` — New `render_vitals_bar()` function, adjust main layout to reserve 1 row at bottom
- `src/tui/viz_viewer/state.rs` — In `tick()` / `on_timer()`, update `last_event_time` from `operations.jsonl` mtime

### Test Strategy

- Unit test: vitals formatting (time-since-last-event display for various durations)
- Unit test: vitals bar renders correctly with 0, 1, N agents
- Integration test: TUI screen dump includes vitals bar content
- Manual: verify vitals update in real time during agent runs

---

## Task 2: Activity Feed Panel

**Priority:** MVP (P1)
**Complexity:** Medium
**TUI Component:** Replace/augment `RightPanelTab::CoordLog` content (`render.rs`, `state.rs`)
**Server Dependency:** None for Phase 1 (tails `operations.jsonl`); event bus enables Phase 2 typed events

### Description

Transform the CoordLog tab from raw daemon log lines into a semantic activity feed showing system-level events (task created, agent spawned, task completed, etc.) in a human-readable, color-coded stream.

### Wireframe

```
┌─ Activity Feed (Coord tab) ─────────────────────────────────────────────┐
│                                                                          │
│  20:04:38  ✓  agent-1234 completed impl-auth (2m30s)                   │
│  20:04:24  ▶  agent-5678 spawned → fix-bug                             │
│  20:03:55  ⟳  coordinator tick: 2 ready, spawning 1                    │
│  20:03:12  +  task "add-tests" created by erik                          │
│  20:02:45  →  fix-bug: open → in-progress                              │
│  20:02:30  ✗  agent-9012 failed on parse-config: test assertion         │
│  20:01:15  ⊘  impl-auth: verification failed (attempt 2/3)             │
│  20:00:58  ✓  impl-auth passed verification                            │
│                                                                          │
│  ─── auto-tail ● (scroll up to pause) ──────────────────────────────────│
└──────────────────────────────────────────────────────────────────────────┘
```

**Event types and formatting:**

| Event | Icon | Color | Source (Phase 1) |
|-------|------|-------|------------------|
| Task created | `+` | Blue | `operations.jsonl`: `op: "create"` |
| Status change | `→` | Yellow | `operations.jsonl`: `op: "status_change"` |
| Agent spawned | `▶` | Green | `daemon.log` parse OR registry diff |
| Agent completed | `✓` | Green bold | `operations.jsonl`: status→done + agent match |
| Agent failed | `✗` | Red bold | `operations.jsonl`: status→failed |
| Coordinator tick | `⟳` | Dim | `daemon.log` parse (already done for CoordLog) |
| Verification result | `⊘`/`✓` | Red/Green | `operations.jsonl`: pending-validation transitions |
| User action | `@` | Cyan | `operations.jsonl`: actor field with WG_USER |

### Implementation Approach

**Phase 1:** Parse `operations.jsonl` into typed `ActivityEvent` structs. On each fs watcher trigger (or periodic poll), read new lines appended since last read position. Format and append to a ring buffer (500 entries max). The CoordLog tab renders this feed instead of (or alongside) raw daemon log.

**Phase 2 upgrade:** When event bus is available, the TUI receives typed events directly. The `ActivityEvent` struct is the same — only the source changes.

### Files Modified

- `src/tui/viz_viewer/state.rs` — Add `ActivityEvent` struct, `activity_feed: VecDeque<ActivityEvent>`, provenance tail position
- `src/tui/viz_viewer/state.rs` — New `reload_activity_feed()` method that tails `operations.jsonl`
- `src/tui/viz_viewer/render.rs` — New `render_activity_feed()` replacing or augmenting `render_coord_log()`
- May add `src/tui/viz_viewer/activity.rs` if parsing logic is substantial

### Test Strategy

- Unit test: `ActivityEvent` parsing from provenance log lines (cover all event types)
- Unit test: ring buffer behavior (overflow, auto-tail, manual scroll pause)
- Unit test: activity feed rendering (each event type produces expected styled line)
- Integration test: create task via CLI → verify activity feed shows it in TUI screen dump

---

## Task 3: Agent Dashboard Tab

**Priority:** MVP (P1)
**Complexity:** Medium
**TUI Component:** New `RightPanelTab::Dashboard` OR repurpose existing agent monitor area
**Server Dependency:** None (reads agent registry + coordinator state, already loaded)

### Description

A dedicated dashboard view showing all running agents, their tasks, elapsed time, token usage, and status. This is the operational nerve center — the first place a user looks to understand "what's happening right now?"

### Wireframe

```
┌─ Dashboard ─────────────────────────────────────────────────────────────┐
│                                                                          │
│  Coordinators                                                            │
│  ┌────────────────────┐ ┌────────────────────┐ ┌────────────────────┐   │
│  │ coord-0 ● Running  │ │ coord-1 ● Running  │ │ coord-2 ○ Idle    │   │
│  │ 2 agents · tick 3s │ │ 1 agent · tick 8s  │ │ 0 agents · 45s    │   │
│  │ 15 tasks managed   │ │ 8 tasks managed    │ │ 3 tasks managed   │   │
│  └────────────────────┘ └────────────────────┘ └────────────────────┘   │
│                                                                          │
│  Active Agents                                                           │
│  ┌──────────┬───────────┬────────────┬────────┬──────────┬───────────┐  │
│  │ Agent    │ Task      │ Elapsed    │ Tokens │ Status   │ Last Out  │  │
│  ├──────────┼───────────┼────────────┼────────┼──────────┼───────────┤  │
│  │ ag-1234  │ impl-auth │ 2m15s      │ 12.3k  │ ● active │ 3s ago   │  │
│  │ ag-5678  │ fix-bug   │ 45s        │ 3.1k   │ ● active │ 1s ago   │  │
│  │ ag-9012  │ add-tests │ 8m30s      │ 45.2k  │ ⚠ slow   │ 35s ago  │  │
│  └──────────┴───────────┴────────────┴────────┴──────────┴───────────┘  │
│                                                                          │
│  Graph Summary                                                           │
│  open: 8 │ in-progress: 3 │ done: 45 │ failed: 1 │ blocked: 2         │
│  activity: ▁▂▅▇█▇▅▃▂▁▁▂▃▅▇  (last 30m)                               │
│                                                                          │
│  [Enter] drill into agent │ [t] task detail │ [k] kill │ [b] back       │
└──────────────────────────────────────────────────────────────────────────┘
```

**Agent status logic:**

| Condition | Display | Color |
|-----------|---------|-------|
| Output received in last 30s | `● active` | Green |
| No output for 30s–5m | `⚠ slow` | Yellow |
| No output for >5m | `⚠ stuck` | Red |
| Process exited | `○ exited` | Dim |

**Data sources (all Phase 1, no event bus needed):**
- Coordinator cards: `coordinator-state.json` (already loaded)
- Agent table: `AgentRegistry` (already loaded) + per-agent output file mtime for "last output" time
- Graph summary: computed from `Graph` (already loaded)
- Activity sparkline: computed from `operations.jsonl` event timestamps

### Files Modified

- `src/tui/viz_viewer/state.rs` — Add `RightPanelTab::Dashboard` variant, dashboard selection state, agent output mtimes
- `src/tui/viz_viewer/render.rs` — New `render_dashboard()` function
- `src/tui/viz_viewer/event.rs` — Dashboard keybindings (Enter for drill-down, k for kill, etc.)
- `src/tui/viz_viewer/state.rs` — `RightPanelTab::ALL` array updated

### Test Strategy

- Unit test: agent status classification (active/slow/stuck thresholds)
- Unit test: dashboard rendering with 0, 1, many agents and coordinators
- Unit test: sparkline computation from event timestamps
- Integration test: screen dump with dashboard tab active shows expected layout

---

## Task 4: Enhanced Toast Notifications

**Priority:** MVP (P1)
**Complexity:** Small
**TUI Component:** Extend existing `self.notification` system (`state.rs`, `render.rs`)
**Server Dependency:** None (Phase 1 triggers from graph diff on reload)

### Description

Upgrade the current single-string notification system to support severity-leveled toasts with configurable display duration and dismissal behavior. Critical for surfacing important events without requiring the user to watch a specific panel.

### Wireframe

```
┌─────────────────────────────────────────────────────────────────────────┐
│ [Graph View]                                                             │
│                                                                          │
│                        ┌──────────────────────────────────────┐          │
│                        │ ✓ impl-auth completed (2m30s)       │ ← info   │
│                        └──────────────────────────────────────┘          │
│                        ┌──────────────────────────────────────┐          │
│                        │ ⚠ agent-9012 may be stuck (5m)      │ ← warn   │
│                        └──────────────────────────────────────┘          │
│                                                                          │
│                                                                          │
└─────────────────────────────────────────────────────────────────────────┘
```

**Toast severity levels:**

| Level | Color | Duration | Auto-dismiss | Example |
|-------|-------|----------|--------------|---------|
| Info | Green | 5s | Yes | Task completed, verification passed |
| Warning | Yellow | 10s | Yes | Agent slow, approaching timeout |
| Error | Red | Until dismissed | No (press Esc) | Task failed, verification failed (final) |

**Phase 1 triggers (detected on graph reload diff):**
- Task status changed to `done` → Info toast
- Task status changed to `failed` → Error toast
- Agent no longer in registry (exited) → Info toast with duration
- Agent output mtime stale (>5m) → Warning toast (deduplicated: once per agent)
- New message for selected task → Info toast

### Files Modified

- `src/tui/viz_viewer/state.rs` — Replace `notification: Option<(String, Instant)>` with `toasts: Vec<Toast>` struct (message, severity, timestamp, dismissed)
- `src/tui/viz_viewer/render.rs` — New `render_toasts()` rendering stacked toasts in top-right corner
- `src/tui/viz_viewer/state.rs` — Toast generation logic in `tick()` / graph diff
- `src/tui/viz_viewer/event.rs` — Esc to dismiss persistent toasts

### Test Strategy

- Unit test: toast lifecycle (creation, auto-expiry by severity, manual dismissal)
- Unit test: toast deduplication (same agent stuck alert doesn't stack)
- Unit test: toast rendering (multiple toasts stack correctly, color per severity)
- Integration test: fail a task → verify error toast appears in screen dump

---

## Task 5: Drill-Down Navigation

**Priority:** MVP (P1)
**Complexity:** Medium
**TUI Component:** Navigation logic connecting Dashboard → Agent → Task → Logs (`event.rs`, `state.rs`)
**Server Dependency:** None (navigates existing TUI panels)

### Description

Implement the navigation chain: Dashboard → select agent → view agent output → jump to task detail → view task logs. Each level provides more detail, and the user can jump back at any point. This ties the dashboard to the existing detail views.

### Wireframe (Navigation Flow)

```
Dashboard (Task 3)
  │
  │  [Enter] on agent row
  ▼
Agent Detail View (existing Output tab, filtered to agent)
  │
  │  [t] task detail
  ▼
Task Detail (existing Detail tab, focused on agent's task)
  │
  │  [l] task log
  ▼
Task Log (existing Log tab, scrolled to task)
  │
  │  [b] or [Esc] at any level
  ▼
Back to previous level (navigation stack)
```

**Navigation stack model:**
```rust
struct NavStack {
    entries: Vec<NavEntry>,
}

enum NavEntry {
    Dashboard,
    AgentDetail { agent_id: String },
    TaskDetail { task_id: String },
    TaskLog { task_id: String },
}
```

Pressing `b` or `Esc` in drill-down context pops the stack and restores the previous view (tab + selection state). This is purely TUI-side navigation — no server interaction.

### Files Modified

- `src/tui/viz_viewer/state.rs` — Add `NavStack` to `VizApp`, push/pop methods
- `src/tui/viz_viewer/event.rs` — Wire Enter (drill in), `b`/Esc (drill out) in dashboard context
- `src/tui/viz_viewer/event.rs` — When drilling to Output tab, set agent filter; when drilling to Detail, set selected task

### Test Strategy

- Unit test: NavStack push/pop behavior, empty stack Esc does nothing
- Unit test: drill-down from dashboard agent row sets correct tab + filter
- Integration test: screen dump sequence through drill-down chain

---

## Task 6: Event Bus TUI Client

**Priority:** Nice-to-have (P2 — Phase 2)
**Complexity:** Medium
**TUI Component:** New async IPC connection in `state.rs`
**Server Dependency:** **Requires mu-plan-server: event-bus task** (Subscribe IPC, broadcast channel)

### Description

Connect the TUI to the daemon's event bus on startup. Receive typed events (`GraphMutated`, `AgentSpawned`, `AgentCompleted`, etc.) and use them for targeted updates instead of full graph reloads.

### Wireframe

No visual change — this is infrastructure. The visible improvement is:
- Activity feed (Task 2) gets events in <50ms instead of polling interval
- Dashboard (Task 3) updates instantly on agent spawn/complete
- Toasts (Task 4) fire within milliseconds of the triggering event

```
┌──────────────────────────────────────────────────────────────────┐
│                     TUI Event Flow                                │
│                                                                    │
│  ┌──────────┐   subscribe    ┌──────────┐                         │
│  │  TUI     │ ──────────────→│  Daemon   │                         │
│  │          │                │  Event    │                         │
│  │          │ ←───────────── │  Bus      │                         │
│  │  routes  │   JSONL stream │           │                         │
│  │  events  │                └──────────┘                         │
│  │  to:     │                                                      │
│  │  • activity_feed.push()                                        │
│  │  • toast_from_event()                                          │
│  │  • targeted_graph_update()                                     │
│  │  • presence_update()                                           │
│  └──────────┘                                                      │
└──────────────────────────────────────────────────────────────────┘
```

**Fallback:** If daemon is not running or doesn't support Subscribe, fall back to Phase 1 polling. The TUI should degrade gracefully.

### Files Modified

- `src/tui/viz_viewer/state.rs` — Add event bus connection (UnixStream), event receiver channel
- `src/tui/viz_viewer/state.rs` — New `connect_event_bus()`, `poll_events()` methods
- `src/tui/viz_viewer/state.rs` — Route events to activity feed, toasts, graph updates
- `src/tui/viz_viewer/event.rs` — Process event channel in main event loop

### Test Strategy

- Unit test: event deserialization for all event types
- Unit test: graceful fallback when daemon socket unavailable
- Unit test: event routing (GraphMutated → graph update, AgentSpawned → dashboard update)
- Integration test: spawn agent via CLI → TUI receives event within 100ms

---

## Task 7: Presence Indicators

**Priority:** Nice-to-have (P2 — Phase 2)
**Complexity:** Small
**TUI Component:** HUD vitals bar addition + task detail annotation (`render.rs`)
**Server Dependency:** **Requires mu-plan-server: presence-protocol** (Presence IPC command, heartbeat tracking)

### Description

Show which users are connected and what they're viewing. Creates ambient awareness of team activity.

### Wireframe

**In vitals bar (Task 1 extension):**
```
● 2 agents │ 8 open · 3 running │ last event 4s ago │ ▲ erik(graph) alice(fix-bug)
```

**In task detail panel:**
```
┌─ Detail: fix-bug ───────────────────────────────────┐
│                                                       │
│  Also viewing: alice                                  │
│                                                       │
│  Status: in-progress                                  │
│  ...                                                  │
└───────────────────────────────────────────────────────┘
```

**In graph view (subtle):**
```
  ┌─fix-bug──────┐
  │ ● in-progress│
  │ 👤 alice     │  ← other user focused on this task
  └──────────────┘
```

**Presence protocol (TUI side):**
1. On startup: send `{"cmd": "presence", "user": "<WG_USER>", "view": "graph", "selected_task": null}`
2. On tab/task selection change: send updated presence
3. Every 30s: heartbeat
4. On exit: send leave (or daemon detects disconnect)

**Privacy:** Respect `wg config --presence off` — if set, don't broadcast and don't render others' presence.

### Files Modified

- `src/tui/viz_viewer/state.rs` — Add `presence_peers: Vec<PresencePeer>` to `VizApp`
- `src/tui/viz_viewer/state.rs` — Send presence updates on selection change, periodic heartbeat
- `src/tui/viz_viewer/render.rs` — Render presence in vitals bar, task detail, graph nodes

### Test Strategy

- Unit test: presence display formatting (multiple users, truncation for narrow terminals)
- Unit test: heartbeat timing (sends every 30s)
- Unit test: privacy config respected (no broadcast when disabled)
- Integration test: two TUI instances → each sees the other in presence bar

---

## Task 8: Surveillance View

**Priority:** Nice-to-have (P2 — Phase 2, builds on Tasks 1–7)
**Complexity:** Medium
**TUI Component:** New TUI mode or full-screen dashboard variant
**Server Dependency:** Best with event bus (Task 6) and presence (Task 7), but functional without

### Description

A birds-eye view optimized for team leads or operators monitoring long-running agent processes. Can be launched as `wg tui --dashboard` or accessed as a full-screen mode within the TUI.

### Wireframe

```
┌──────────────────────────────────────────────────────────────────────────┐
│ WORKGRAPH SURVEILLANCE                      ▲ 3 users │ uptime 4h22m    │
├──────────────────────────────────────────────────────────────────────────┤
│                                                                          │
│  Coordinators           │  Active Agents                                 │
│  ┌───────────────────┐  │  ag-1234 │ impl-auth │ 2m15s │ ● active      │
│  │ coord-0  ● 2 agt  │  │  ag-5678 │ fix-bug   │ 45s   │ ● active      │
│  │ coord-1  ● 1 agt  │  │  ag-9012 │ add-tests │ 8m30s │ ⚠ slow        │
│  │ coord-2  ○ idle   │  │                                                │
│  └───────────────────┘  │                                                │
│                          │                                                │
│  Graph Health            │  Recent Events                                │
│  ████████░░  80% done   │  20:04:38 ✓ impl-auth completed              │
│  open: 8 │ run: 3       │  20:04:24 ▶ fix-bug spawned                  │
│  done: 45 │ fail: 1     │  20:03:55 ⟳ coord-0 tick                     │
│                          │  20:03:12 + add-tests created                 │
│  Activity (30m)          │  20:02:45 → fix-bug: open→running            │
│  ▁▂▅▇█▇▅▃▂▁▁▂▃▅▇       │                                                │
│                          │                                                │
│  Alerts                  │                                                │
│  ⚠ ag-9012 slow (35s)  │                                                │
│                          │                                                │
├──────────────────────────────────────────────────────────────────────────┤
│ [d]rill agent │ [t]ask │ [l]ogs │ [a]lerts │ [f]ull graph │ [q]uit      │
└──────────────────────────────────────────────────────────────────────────┘
```

This is essentially a composition of Tasks 1 (vitals), 2 (activity feed), 3 (agent dashboard), and 4 (toasts/alerts) into a single full-screen layout. The implementation reuses render functions from those tasks.

### Files Modified

- `src/tui/viz_viewer/state.rs` — Add `surveillance_mode: bool` or `ViewMode::Surveillance` enum
- `src/tui/viz_viewer/render.rs` — New `render_surveillance()` composing existing sub-renderers
- `src/tui/viz_viewer/event.rs` — Surveillance-mode keybindings, mode toggle (e.g., F5 or `wg tui --dashboard`)
- `src/tui/mod.rs` — `--dashboard` CLI flag to launch directly in surveillance mode

### Test Strategy

- Unit test: surveillance layout renders all four quadrants
- Unit test: mode toggle preserves state when switching back to normal view
- Integration test: `wg tui --dashboard` launches in surveillance mode
- Manual: run 3+ agents, verify surveillance view updates in real time

---

## Priority Summary

### MVP (P1) — No server dependency, ship with or before mu-plan-server

| # | Task | Complexity | Estimated Lines |
|---|------|-----------|----------------|
| 1 | HUD Vitals Bar | S | ~200-300 |
| 2 | Activity Feed Panel | M | ~400-600 |
| 3 | Agent Dashboard Tab | M | ~500-700 |
| 4 | Enhanced Toast Notifications | S | ~200-300 |
| 5 | Drill-Down Navigation | M | ~300-400 |

**MVP Total:** ~1600-2300 lines of Rust. 5 tasks, parallelizable as:
- Tasks 1, 4 can run in parallel (independent areas)
- Task 2 can run in parallel with Task 3 (different tabs)
- Task 5 depends on Task 3 (needs dashboard to drill into)

### Nice-to-Have (P2) — Requires mu-plan-server event infrastructure

| # | Task | Complexity | Estimated Lines |
|---|------|-----------|----------------|
| 6 | Event Bus TUI Client | M | ~400-600 |
| 7 | Presence Indicators | S | ~200-300 |
| 8 | Surveillance View | M | ~400-500 |

**P2 Total:** ~1000-1400 lines of Rust. Sequential dependency: Task 6 → Tasks 7, 8.

### Implementation Order (Recommended)

```
Week 1:  Task 1 (vitals) ──────────────┐
         Task 4 (toasts)  ──────────────┤ parallel
         Task 2 (activity feed) ────────┘
                                         
Week 2:  Task 3 (dashboard) ───────────┐
         Task 5 (drill-down) ──────────┘ sequential (5 after 3)

--- MVP complete ---

Week 3+: Task 6 (event bus client) ────┐ blocked on mu-plan-server
         Task 7 (presence) ────────────┘ after Task 6
         Task 8 (surveillance) ─────────  after Tasks 6+7
```

---

## Shared Infrastructure Needed

These utilities serve multiple tasks and should be extracted as shared code:

1. **Provenance log tailer** — Incremental reader for `operations.jsonl` (used by Tasks 1, 2, 3)
2. **Agent status classifier** — `active/slow/stuck` logic from output mtime (used by Tasks 3, 4, 8)
3. **Sparkline widget** — Ratatui sparkline from event rate data (used by Tasks 1, 3, 8)
4. **Relative time formatter** — `"4s ago"`, `"2m30s"` etc. (used everywhere)

---

## Risk Factors

| Risk | Mitigation |
|------|-----------|
| `operations.jsonl` grows large → slow tail | Track file offset; only read new bytes since last poll |
| Too many toasts overwhelm the screen | Max 4 visible toasts; oldest auto-dismissed; deduplication |
| Dashboard tab makes RightPanelTab enum large (11 variants) | Acceptable; the tab bar already handles 10. Consider hiding low-use tabs |
| Surveillance mode duplicates render code | Compose from shared sub-renderers; no copy-paste |
| Event bus protocol changes during mu-plan-server dev | TUI client uses typed event enum with `#[serde(other)]` for forward compat |
