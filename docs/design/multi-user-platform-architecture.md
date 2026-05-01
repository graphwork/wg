# Unified Multi-User Platform Architecture

**Task:** mu-design-synthesis
**Date:** 2026-03-25
**Status:** Synthesis of four design explorations
**Inputs:**
- [TUI Multiplexing & Concurrent Access](tui-multiplexing-concurrent-access.md) (mu-design-tui-concurrency)
- [Terminal Wrapping Strategy](terminal-wrapping-strategy.md) (mu-design-terminal-wrapping)
- [Federation & Cross-Workgraph Visibility](federation-architecture.md) (mu-design-federation)
- [Real-Time Sync & Liveness UX](live-sync-and-liveness.md) (mu-design-live-sync)

---

## Executive Summary

Workgraph's multi-user platform extends the existing single-user tool into a shared workspace where multiple humans and AI agents coordinate through a common graph. The architecture is built on four principles:

1. **The graph is the source of truth.** All coordination flows through `graph.jsonl`, serialized by flock.
2. **The TUI is the universal interface.** Every platform (desktop, web, mobile) connects to the same TUI via terminal wrapping — no platform-specific frontends.
3. **The daemon is the event hub.** The service daemon manages coordination (agents, presence, events, federation) for all connected users.
4. **Observation before mutation.** Federation starts read-only; writes come later. Liveness starts with local sync; distributed sync comes later.

The MVP (v0.1) requires ~2 weeks of focused work: completing the `modify_graph()` migration, adding `WG_USER` identity, per-user coordinators, and web access via ttyd. No architectural changes to the graph model.

---

## 1. Layered Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                       PRESENTATION LAYER                                │
│                                                                         │
│   Desktop        Web            Android          iOS                    │
│   SSH/mosh  →   ttyd/Caddy  →  Termux/mosh  →  Blink/mosh             │
│        \           |              /               /                      │
│         └──────────┼─────────────┘───────────────┘                      │
│                    ▼                                                     │
│              tmux (per-user session)                                     │
│                    │                                                     │
│              ┌─────▼──────┐                                             │
│              │   wg tui   │  (N instances, one per user)                │
│              └─────┬──────┘                                             │
├────────────────────┼────────────────────────────────────────────────────┤
│                    │         SESSION LAYER                               │
│                    ▼                                                     │
│   ┌────────────────────────────────────────────────────────────┐        │
│   │              Service Daemon (wg service)                    │        │
│   │                                                             │        │
│   │  ┌──────────────┐  ┌───────────────┐  ┌────────────────┐  │        │
│   │  │ Coordinator   │  │ Event Bus     │  │ Federation     │  │        │
│   │  │ (per-user)    │  │ (pub-sub)     │  │ Poller         │  │        │
│   │  ├──────────────┤  ├───────────────┤  ├────────────────┤  │        │
│   │  │ Agent Spawner │  │ Presence      │  │ Alert Monitor  │  │        │
│   │  └──────────────┘  └───────────────┘  └────────────────┘  │        │
│   │                                                             │        │
│   │  IPC: Unix socket (.wg/service/daemon.sock)         │        │
│   └────────────────────────────────────────────────────────────┘        │
├─────────────────────────────────────────────────────────────────────────┤
│                         SYNC LAYER                                      │
│                                                                         │
│   Local (single machine):                                               │
│     Phase 1: fs watcher (inotify) + flock ──── <100ms propagation      │
│     Phase 2: Event bus (typed IPC push) ─────── <50ms propagation       │
│                                                                         │
│   Distributed (multi-machine):                                          │
│     Phase 3: Git merge driver ───────────────── <30s propagation        │
│     Phase 4: Operation-log CRDT ─────────────── <100ms propagation      │
│                                                                         │
├─────────────────────────────────────────────────────────────────────────┤
│                         GRAPH LAYER                                     │
│                                                                         │
│   .wg/graph.jsonl   (shared state, single file, JSONL)           │
│   .wg/operations.jsonl  (provenance log, append-only)            │
│   .wg/service/     (daemon state, agent registry)                │
│   .wg/chat/        (coordinator chat, per-ID)                    │
│   .wg/agency/      (roles, tradeoffs, agents)                    │
│                                                                         │
│   Primitives:                                                           │
│     modify_graph()  ─── exclusive flock, read-modify-write, atomic      │
│     load_graph()    ─── shared flock (non-blocking), consistent reads   │
│     atomic rename   ─── readers never see partial writes                │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

### Layer Responsibilities and Interfaces

| Layer | Responsibility | Upward Interface | Downward Interface |
|-------|---------------|------------------|-------------------|
| **Graph** | Persistent state, conflict-free writes, audit trail | `modify_graph()`, `load_graph()`, provenance log | Filesystem (JSONL files, flock) |
| **Sync** | Change propagation between consumers | Events: `GraphMutated`, `AgentSpawned`, etc. | fs watcher (inotify), graph mtime, IPC |
| **Session** | User coordination, agent lifecycle, federation | IPC protocol (Unix socket), presence, alerts | Sync layer events, graph layer mutations |
| **Presentation** | User interaction, visualization | Terminal I/O (stdin/stdout via PTY) | Session layer IPC, graph layer reads |

### Interface Contracts

**Graph → Sync:** Every `modify_graph()` call triggers an `IN_MOVED_TO` inotify event (via atomic rename). In Phase 2+, the daemon also publishes a typed `GraphMutated` event on the event bus.

**Sync → Session:** The daemon subscribes to sync events (fs watcher in Phase 1, event bus in Phase 2). On each event, it evaluates: coordinator tick needed? Alert condition triggered? Presence update? Federation snapshot stale?

**Session → Presentation:** TUI instances connect to the daemon via IPC socket. They subscribe to events (Phase 2) or poll the filesystem directly (Phase 1). The daemon provides presence state, federation snapshots, and alert notifications.

**Presentation → User:** Terminal I/O flows through tmux sessions, wrapped by SSH/mosh (desktop/mobile) or ttyd/WebSocket (web). The TUI renders identically regardless of transport.

---

## 2. Cross-Cutting Concerns Reconciled

### 2.1 User Identity

All four designs converge on `WG_USER` as the identity mechanism:

| Design | Uses WG_USER For |
|--------|-----------------|
| TUI Concurrency | Chat attribution, coordinator ownership, provenance |
| Terminal Wrapping | tmux session naming (`$USER-wg`), ttyd session binding |
| Federation | Origin tracking on cross-repo tasks |
| Live Sync | Presence indicators, activity feed attribution |

**Unified identity model:**

```
WG_USER (env var, string)
  │
  ├── Falls back to $USER (Unix username)
  ├── Falls back to "unknown"
  │
  ├── Used by: provenance log, chat messages, coordinator labels
  ├── Used by: tmux session naming (presentation layer)
  ├── Used by: presence tracking (session layer)
  ├── Used by: federation origin (session layer)
  └── NOT used for: access control (advisory, not enforced)
```

**Conflict resolved:** The terminal wrapping design uses `$USER` (Unix username) for tmux sessions. The TUI concurrency design uses `WG_USER` for coordinator labels. These can differ (e.g., shared account with distinct `WG_USER`). The tmux session name should use `WG_USER` when set, falling back to `$USER`:

```bash
tmux new-session -A -s "${WG_USER:-$USER}-wg" "wg tui"
```

### 2.2 Authentication

The four designs address authentication at different layers:

| Layer | Mechanism | Design Source |
|-------|-----------|--------------|
| Transport (SSH) | SSH keys | Terminal Wrapping |
| Transport (Web) | TLS + OAuth2/basic auth via reverse proxy | Terminal Wrapping |
| IPC (same machine) | Unix socket permissions (0600/0660) | Federation |
| IPC (multi-machine) | TLS + token auth (future) | Federation |
| Data (federation) | Visibility filtering (internal/peer/public) | Federation |

**No contradiction.** Authentication is layered: transport auth gets you a session; IPC perms control daemon access; visibility filtering controls data exposure. Each layer is independent.

**The reverse proxy is the auth gateway for web users.** Workgraph itself does not implement user authentication. This is deliberate — auth is a solved problem (Caddy, OAuth2 Proxy, Authelia). Workgraph delegates to external auth and trusts the `WG_USER` identity passed downstream.

### 2.3 The Daemon as Central Hub

Three of four designs rely on the daemon for their core functionality:

| Design | Daemon Role |
|--------|-------------|
| TUI Concurrency | Coordinator lifecycle, agent spawning, task claiming |
| Federation | Handles `QueryGraph` IPC, peer state caching |
| Live Sync | Event bus host, presence tracking, alert monitoring |

**Shared infrastructure:** The daemon's IPC socket (`daemon.sock`) is the single integration point. All session-layer services (event bus, presence, federation, coordinators) run as subsystems within the same daemon process.

**Conflict resolved:** The federation design proposes polling peers on a 30s interval. The live sync design wants <100ms local propagation. These are different scopes — federation polling is for cross-graph observation (inherently slower), while local sync is for within-graph consistency (inherently fast). No conflict; both coexist in the daemon.

### 2.4 The fs Watcher: Shared Foundation

Both TUI concurrency and live sync rely on the `notify` fs watcher:

- **TUI concurrency:** fs watcher triggers graph reload when any TUI or CLI mutates the graph
- **Live sync:** fs watcher is the Phase 1 sync mechanism; the event bus (Phase 2) replaces it for subscribed TUIs

**Evolution path:** Phase 1 uses N independent fs watchers (one per TUI instance). Phase 2 consolidates to 1 watcher in the daemon, broadcasting typed events to N subscribers. This reduces inotify load by Nx and enables targeted updates instead of full reloads.

**The fs watcher remains as fallback** for standalone TUI mode (no daemon running). This ensures the TUI works without the service — important for simple single-user use.

### 2.5 tmux as Universal Session Layer

The terminal wrapping design identifies tmux as the linchpin. This intersects with other designs:

| Concern | tmux's Role |
|---------|------------|
| Session persistence | TUI survives disconnects (all platforms) |
| Multi-device access | Same user can attach from desktop AND phone |
| Window management | Users run `wg` CLI alongside TUI |
| Presence boundary | tmux attach/detach events could drive presence tracking |

**Integration point:** The daemon could detect tmux session events (via `tmux list-sessions`) to infer user presence without requiring explicit IPC heartbeats. This is a Phase 2 optimization — for Phase 1, the TUI sends explicit heartbeats.

---

## 3. Conflict Resolution Matrix

| Conflict | Resolution | Rationale |
|----------|-----------|-----------|
| **fs watcher vs event bus for sync** | Sequential phases, not alternatives. Phase 1: fs watcher. Phase 2: event bus (fs watcher as fallback). | Event bus requires daemon; fs watcher works standalone. Ship the simple thing first. |
| **Polling (federation) vs push (local sync)** | Different mechanisms for different scopes. Local: push (<100ms). Cross-graph: poll (30s). | Cross-graph push would require persistent connections between daemons — overkill for v0.1. |
| **tmux session name: $USER vs WG_USER** | Use `${WG_USER:-$USER}` everywhere. | Consistent identity across all layers. |
| **No CRDT (TUI concurrency) vs CRDT as Phase 4 (live sync)** | No conflict — CRDT is explicitly deferred in both designs. Single-machine flock is sufficient for the target scale (≤10 users). | CRDT only needed for real-time multi-machine sync, which is Phase 4. |
| **Auth at reverse proxy (wrapping) vs Unix perms (federation)** | Layered auth: reverse proxy for web transport, Unix perms for IPC, visibility for data. | Defense in depth. Each layer handles its own access control. |
| **Activity feed (live sync) vs Coord Log (existing)** | Activity feed replaces Coord Log as a TUI tab. Coord Log raw output available via scrollback. | Activity feed is the semantic, user-facing view. Raw logs are for debugging. |
| **Responsive TUI (wrapping) vs current fixed layout (TUI)** | Responsive breakpoints required before mobile is viable. Separate implementation task. | Mobile access is useless without small-screen support. |
| **Per-user coordinator state files vs shared coordinator-state.json** | Per-coordinator files (`coordinator-state-{id}.json`). | Eliminates write contention between coordinators updating their own state. |

---

## 4. Shared Infrastructure Map

Components that serve multiple subsystems:

```
┌─────────────────────────────────────────────────────────────────┐
│                   SHARED INFRASTRUCTURE                          │
│                                                                  │
│  ┌──────────────────┐    Serves:                                │
│  │ modify_graph()   │    • TUI concurrency (write serialization)│
│  │ (flock + atomic  │    • Live sync (triggers inotify events)  │
│  │  rename)          │    • Federation (read-only via load_graph)│
│  └──────────────────┘                                           │
│                                                                  │
│  ┌──────────────────┐    Serves:                                │
│  │ Daemon IPC socket│    • Event bus (live sync)                │
│  │ (daemon.sock)    │    • Federation (QueryGraph)              │
│  │                  │    • Presence (live sync)                  │
│  │                  │    • Coordinator management                │
│  └──────────────────┘                                           │
│                                                                  │
│  ┌──────────────────┐    Serves:                                │
│  │ fs watcher       │    • TUI refresh (concurrency)            │
│  │ (inotify/notify) │    • Sync layer (Phase 1 change detection)│
│  │                  │    • Daemon event generation               │
│  └──────────────────┘                                           │
│                                                                  │
│  ┌──────────────────┐    Serves:                                │
│  │ WG_USER identity │    • Provenance (audit trail)             │
│  │                  │    • Chat attribution (TUI concurrency)   │
│  │                  │    • Presence (live sync)                  │
│  │                  │    • Federation origin (cross-repo tasks)  │
│  │                  │    • tmux session naming (wrapping)        │
│  └──────────────────┘                                           │
│                                                                  │
│  ┌──────────────────┐    Serves:                                │
│  │ tmux             │    • Session persistence (all platforms)   │
│  │                  │    • Multi-device access                   │
│  │                  │    • Presence inference (future)           │
│  └──────────────────┘                                           │
│                                                                  │
│  ┌──────────────────┐    Serves:                                │
│  │ Notification     │    • Alerts (live sync)                   │
│  │ router           │    • Federation events (future)           │
│  │ (notify/mod.rs)  │    • Mobile push (when not at TUI)        │
│  └──────────────────┘                                           │
│                                                                  │
│  ┌──────────────────┐    Serves:                                │
│  │ Provenance log   │    • Audit trail (all writes)             │
│  │ (operations.jsonl)│   • Activity feed (live sync)            │
│  │                  │    • CRDT seed data (future Phase 4)      │
│  └──────────────────┘                                           │
└─────────────────────────────────────────────────────────────────┘
```

---

## 5. Phased Implementation Roadmap

### v0.1 — Multi-User Foundation (MVP)

**Goal:** Multiple users on a shared VPS can each run `wg tui`, see each other's effects, and manage their own agents. Web access works via ttyd.

**Scope:** ~1500 lines of Rust + deployment docs. Estimated: 2 weeks.

| Work Item | Source Design | Effort | Priority |
|-----------|--------------|--------|----------|
| Complete `modify_graph()` migration for all mutation commands | TUI Concurrency | Medium | P0 — prerequisite |
| Agent registry: universal `load_locked()` | TUI Concurrency | Small | P0 — prerequisite |
| `WG_USER` env var support in provenance, logs, chat | TUI Concurrency | Small | P0 |
| Per-user coordinator creation from `WG_USER` | TUI Concurrency | Small | P0 |
| Coordinator state: per-ID files | TUI Concurrency | Small | P1 |
| Chat inbox flock protection | TUI Concurrency | Small | P1 |
| Chat message `user` field | TUI Concurrency | Small | P1 |
| ttyd + Caddy deployment guide | Terminal Wrapping | Small | P1 |
| Test TUI rendering in xterm.js | Terminal Wrapping | Small | P1 |
| `wg peer scan` command | Federation | Small | P1 |
| Federation config in config.toml | Federation | Small | P1 |
| Liveness: "time since last event" in HUD | Live Sync | Small | P1 |
| Liveness: agent elapsed time in graph view | Live Sync | Small | P1 |

**What v0.1 delivers:**
- 2-7 users on a shared VPS, each with SSH access, each running `wg tui`
- Changes propagate via fs watcher in <100ms
- Each user has their own coordinator and agent budget
- Web access via ttyd (zero code changes to workgraph)
- Graph mutations are safe (flock-serialized, TOCTOU-free)
- Provenance tracks who did what

**What v0.1 explicitly does NOT include:**
- Event bus (fs watcher is sufficient for ≤7 users)
- Presence indicators (users see effects, not cursors)
- Federation visibility in TUI (CLI-only peer commands)
- Responsive TUI for mobile
- Distributed sync

### v0.2 — Liveness & Federation Visibility

**Goal:** The system feels alive. Users see presence, activity feeds, and peer workgraph state in the TUI.

**Scope:** ~3000 lines of Rust. Estimated: 4-6 weeks.

| Work Item | Source Design | Effort |
|-----------|--------------|--------|
| Event bus (pub-sub in daemon IPC) | Live Sync | Medium |
| TUI event bus client + targeted refresh | Live Sync | Medium |
| Presence protocol (IPC heartbeat) | Live Sync | Small |
| Presence display in HUD | Live Sync | Small |
| Activity feed panel (replaces Coord Log) | Live Sync | Medium |
| Activity sparkline in HUD | Live Sync | Small |
| Enhanced toast notifications (severity levels) | Live Sync | Small |
| `QueryGraph` IPC request + handler | Federation | Small |
| Visibility filtering in QueryGraph | Federation | Small |
| `wg peer tasks` CLI command | Federation | Small |
| TUI Peers tab (list + drill-down) | Federation | Medium |
| Snapshot caching (TTL) | Federation | Small |
| Dashboard tab (surveillance view) | Live Sync | Medium |

**What v0.2 delivers:**
- Event-driven TUI updates (<50ms latency)
- "Who's online" presence indicators
- Semantic activity feed (not raw logs)
- Federation: see peer workgraph state from TUI
- Surveillance dashboard for team leads
- Alert monitoring with notification routing

### v0.3 — Mobile & Distributed

**Goal:** Mobile access is genuinely usable. Multi-machine collaboration works via git.

**Scope:** ~2000 lines of Rust + setup guides. Estimated: 4-6 weeks.

| Work Item | Source Design | Effort |
|-----------|--------------|--------|
| Responsive TUI breakpoints (<50, 50-80, >80 cols) | Terminal Wrapping | Medium |
| Single-panel mode for narrow terminals | Terminal Wrapping | Medium |
| Termux setup guide + script | Terminal Wrapping | Small |
| Blink Shell configuration guide | Terminal Wrapping | Small |
| PWA manifest for ttyd | Terminal Wrapping | Small |
| Git merge driver for graph.jsonl | Live Sync | Medium |
| `wg sync` convenience command | Live Sync | Small |
| Cross-repo task dispatch (`--repo` flag) | Federation | Medium |
| Cross-repo dependencies (`peer:task-id`) | Federation | Medium |
| TUI inline federation markers | Federation | Small |
| Push notifications between peers (GraphChanged) | Federation | Small |

**What v0.3 delivers:**
- Mobile-friendly TUI layout
- One-command setup for Android (Termux) and iOS (Blink Shell)
- PWA for "Add to Home Screen" on mobile browsers
- Multi-machine collaboration via git with smart merging
- Cross-repo task dispatch and dependencies

### v1.0 — Full Multi-User Platform

**Goal:** Production-grade multi-user, multi-machine platform.

| Work Item | Source Design | Effort |
|-----------|--------------|--------|
| TCP IPC with TLS (multi-machine federation) | Federation | High |
| Token-based auth for TCP transport | Federation | Medium |
| Operation-log CRDT storage model | Live Sync | High |
| Replication protocol (TCP/WebSocket) | Live Sync | High |
| Global `max_total_agents` cap | TUI Concurrency | Small |
| mDNS/DNS-SD service announcement | Federation | Medium |
| Custom Android app (if demand warrants) | Terminal Wrapping | High |
| Custom iOS app (if demand warrants) | Terminal Wrapping | High |
| Federated agency sync (auto-pull on tick) | Federation | Medium |

---

## 6. Migration Path: Single-User → Multi-User

### Step 0: Current State (Single-User)

```
Single user → single TUI → single coordinator → agents
All state in .wg/, no contention, no identity
```

### Step 1: Harden Foundations (v0.1 prerequisite)

Before any multi-user deployment:

1. **Audit and migrate all mutation commands to `modify_graph()`.**
   The file locking audit (`docs/research/file-locking-audit.md`) identifies commands still using the old `load_graph()` + `save_graph()` pattern. Each is a TOCTOU race under concurrent access. This migration is the single most important prerequisite.

2. **Audit agent registry locking.** Ensure all registry read-modify-write operations use `load_locked()`. Multiple coordinators spawning agents concurrently will race without this.

3. **Add `WG_USER` support.** Thread the env var through provenance, chat, and coordinator labels. Fallback chain: `WG_USER` → `$USER` → `"unknown"`.

### Step 2: Deploy Multi-User (v0.1)

1. Shared VPS with Unix accounts per user (SSH key auth)
2. Each user's shell profile sets `export WG_USER="<name>"`
3. Each user runs: `tmux new-session -A -s "${WG_USER}-wg" "wg tui"`
4. First TUI launch auto-creates a coordinator labeled with `WG_USER`
5. Web access: deploy ttyd + Caddy (see deployment guide)

**Zero graph format changes.** The existing `graph.jsonl` format is multi-user compatible. New fields (`user` on chat messages, `label` on coordinator state) are additive.

### Step 3: Enable Liveness (v0.2)

1. Upgrade workgraph binary (the daemon gains event bus + presence)
2. Restart `wg service` — TUI instances auto-connect to event bus
3. Configure notification channels in `config.toml` for alerts
4. Add peers via `wg peer add` for federation visibility

### Step 4: Go Mobile (v0.3)

1. Install responsive TUI update
2. Share Termux/Blink setup guides with mobile users
3. Enable git merge driver for multi-machine teams
4. Configure cross-repo dependencies if using federated workgraphs

### Rollback Safety

Every step is backward-compatible:
- v0.1 changes are additive (new env var, new state files). Removing `WG_USER` falls back to `$USER`.
- v0.2 event bus is opt-in. TUI without daemon falls back to fs watcher.
- v0.3 git merge driver is registered in `.gitattributes` — removing it falls back to normal git merge (line-level conflicts).
- No graph format migrations. No schema changes. No data loss risk.

---

## 7. Mapping to Existing Code

### What Already Works (No Changes Needed)

| Capability | Location | Multi-User Status |
|------------|----------|------------------|
| Multiple TUI instances on same `.wg/` | `src/tui/` | Works today |
| fs watcher with 50ms debounce | `state.rs:3857` | Works today |
| `modify_graph()` flock serialization | `parser.rs:267` | Works for migrated commands |
| Atomic read consistency (rename) | `parser.rs` | Works today |
| Per-instance view state (cursor, scroll) | `VizApp` in-memory | Works today |
| Multiple coordinators via tab bar | `state.rs:2197`, `render.rs:1896` | Works today |
| Peer config + commands | `src/federation.rs`, `src/commands/peer.rs` | Works today |
| Agency federation (pull/push/scan) | `src/federation.rs` | Works today |
| IPC `AddTask` + `QueryTask` | `src/commands/service/ipc.rs` | Works today |
| Task visibility field | `src/graph.rs:303` | Works today |
| Notification router | `src/notify/mod.rs` | Works today |
| Provenance log | `src/provenance.rs` | Works today |
| Termux detection | `event.rs:48-50` | Works today |
| Screen dump IPC | `screen_dump.rs` | Works today |

### What Needs Changes (by priority)

**P0 — Required before multi-user deployment:**

| Change | Files | Effort | Design Source |
|--------|-------|--------|--------------|
| Complete `modify_graph()` migration | ~10 command files | Medium | TUI Concurrency §3.3 |
| Universal `load_locked()` for agent registry | `service/spawn.rs` | Small | TUI Concurrency §6.2 |
| `WG_USER` in provenance + logs | `provenance.rs`, `graph.rs`, log cmds | Small | TUI Concurrency §7 |

**P1 — Quality of life for v0.1:**

| Change | Files | Effort | Design Source |
|--------|-------|--------|--------------|
| Per-coordinator state files | `service/mod.rs` | Small | TUI Concurrency §6.2 |
| Chat inbox flock | `chat.rs` | Small | TUI Concurrency §6.2 |
| Auto-create coordinator from `WG_USER` | `tui/viz_viewer/state.rs` | Small | TUI Concurrency §5.2 |
| Chat `user` field | `chat.rs`, TUI render | Small | TUI Concurrency §4.3 |
| Agent elapsed time in graph nodes | `render.rs` | Small | Live Sync §3.4 |
| "Last event" counter in HUD | `state.rs`, `render.rs` | Small | Live Sync §3.3 |
| `wg peer scan` command | `commands/peer.rs` | Small | Federation §2.1 |

**P2 — Event bus and liveness (v0.2):**

| Change | Files | Effort | Design Source |
|--------|-------|--------|--------------|
| `Subscribe` IPC command + broadcast | `service/ipc.rs`, `service/mod.rs` | Medium | Live Sync §2.2 |
| TUI event bus client | `tui/viz_viewer/state.rs` | Medium | Live Sync §2.2 |
| Presence protocol | `service/ipc.rs`, `state.rs` | Small | Live Sync §3.2 |
| Activity feed panel | `tui/viz_viewer/render.rs` | Medium | Live Sync §3.1 |
| `QueryGraph` IPC handler | `service/ipc.rs` | Small | Federation §4.3 |
| Visibility filtering | `service/ipc.rs`, `graph.rs` | Small | Federation §3 |
| TUI Peers tab | `tui/viz_viewer/` | Medium | Federation §7 |
| Dashboard tab | `tui/viz_viewer/` | Medium | Live Sync §4 |

**P3 — Mobile and distributed (v0.3):**

| Change | Files | Effort | Design Source |
|--------|-------|--------|--------------|
| Responsive TUI breakpoints | `render.rs` | Medium | Terminal Wrapping §2.2 |
| Git merge driver | New binary: `wg merge-driver` | Medium | Live Sync §2.3 |
| Cross-repo dispatch (`--repo`) | `commands/add.rs`, IPC | Medium | Federation §6 |
| Cross-repo dependencies | `graph.rs`, `commands/service/coordinator.rs` | Medium | Federation §6 |

---

## 8. Scalability Envelope

### Target Operating Range

| Metric | v0.1 | v0.2 | v0.3 | v1.0 |
|--------|------|------|------|------|
| Concurrent users | 2-7 | 5-10 | 5-10 | 10-20+ |
| Concurrent writers | 2-5 | 5-10 | 5-10 | 10-20+ |
| Write throughput | 20-50/s | 20-50/s | 20-50/s | Batched: 100+/s |
| Local sync latency | <100ms | <50ms | <50ms | <50ms |
| Cross-graph sync | N/A | 30s polling | 30s polling | <5s push |
| Distributed sync | N/A | N/A | <30s (git) | <100ms (CRDT) |
| Federated peers | 0 | 2-10 | 2-10 | 10+ |
| Graph size (tasks) | 1-1000 | 1-5000 | 1-5000 | 10000+ |

### Scaling Bottlenecks and Mitigations

| Bottleneck | Threshold | Mitigation | Phase |
|------------|-----------|-----------|-------|
| Write contention (flock) | >10 concurrent writers | Daemon mutation queue (batch under single flock) | v1.0 |
| inotify storm | >20 fs watchers | Event bus replaces per-instance watchers | v0.2 |
| Graph file size | >5MB | Compaction (already exists as `.compact-0`) | Now |
| Full graph reload | >500ms parse | Event bus enables incremental update | v0.2 |
| Federation polling cost | >10 peers at 30s | Snapshot caching (5s TTL), active-tab-only polling | v0.2 |

---

## 9. Security Model

### Defense in Depth

```
Layer 1: Transport Authentication
  ├── SSH: key-based auth (desktop, mobile)
  ├── Web: TLS + OAuth2/basic auth (reverse proxy)
  └── Scope: who can connect to the server

Layer 2: Operating System Permissions
  ├── .wg/ directory: owned by project group
  ├── daemon.sock: 0660 (group-readable)
  ├── graph.jsonl: 0640 (group-readable)
  └── Scope: who can access the workgraph on the machine

Layer 3: Data Visibility Filtering
  ├── internal: never exposed to peers (default)
  ├── peer: visible to configured peers (filtered view)
  ├── public: visible to anyone (sanitized view)
  └── Scope: what data crosses federation boundaries

Layer 4: Write Protection
  ├── modify_graph(): flock serialization prevents corruption
  ├── AddTask IPC: rate-limited (100/s per connection)
  ├── Origin tracking: cross-repo tasks record source
  └── Scope: preventing destructive writes
```

### Trust Model

**Within a workgraph:** Full trust. All users can read and modify all tasks. The system is collaborative, not adversarial. `WG_USER` is advisory (not enforced). Provenance provides accountability, not access control.

**Across workgraphs (federation):** Trust is established by explicit peer configuration (`wg peer add`). Visibility filtering ensures internal tasks are never exposed. Read operations are safe (can't break anything). Write operations (`AddTask`) are opt-in and rate-limited.

**The system is designed for small teams (2-10 people) on a shared VPS.** It is not designed for untrusted multi-tenant environments. Adding per-task ACLs or role-based access control is a v2.0 concern, if ever needed.

---

## 10. Architecture Decision Records

### ADR-S1: Single Shared Workgraph per Project

**Decision:** All users in a project operate on one `.wg/graph.jsonl`.
**Sources:** TUI Concurrency ADR-1, Federation §6.
**Rationale:** Simplest model. Already works. Federation provides cross-project visibility without merging graphs. Per-user workgraphs would require federation for basic task visibility within a team.
**Consequence:** No per-user access control within a project. Acceptable for small teams.

### ADR-S2: Daemon as Single Event Hub

**Decision:** All session-layer services (event bus, presence, federation, coordinators) run within the service daemon process.
**Sources:** Live Sync §7 Decision 1, Federation §4.2, TUI Concurrency §5.
**Rationale:** The daemon already manages IPC, agents, and coordinators. Adding subsystems to the daemon reuses the IPC socket and avoids new processes. A separate event bus process adds operational complexity without architectural benefit.
**Consequence:** Daemon restart disrupts all services. Acceptable — workgraph is not a high-availability system.

### ADR-S3: tmux as Universal Session Layer

**Decision:** All platforms connect through tmux sessions.
**Sources:** Terminal Wrapping §3.2, TUI Concurrency §4.2.
**Rationale:** tmux provides session persistence (survives disconnects), named sessions (prevents collisions), and window management (CLI alongside TUI). It's the linchpin that makes "the TUI is the universal interface" work.
**Consequence:** tmux is a required server-side dependency. Acceptable — it's universally available and well-maintained.

### ADR-S4: No CRDT Until Multi-Machine Real-Time is Required

**Decision:** Keep flock-serialized single-file storage through v0.3. CRDT is v1.0+ only.
**Sources:** TUI Concurrency ADR-5, Live Sync §2.3.
**Rationale:** Flock provides linearizable writes at 20-50 writes/sec, sufficient for ≤10 concurrent writers on a single machine. Git merge driver handles multi-machine sync for v0.3. CRDT adds massive complexity (vector clocks, operation logs, replication protocol) with no benefit until real-time multi-machine sync is needed.
**Consequence:** Multi-machine collaboration is eventually-consistent (git-based) until v1.0.

### ADR-S5: Observation-First Federation

**Decision:** Federation starts read-only. Cross-repo writes exist (via AddTask IPC) but are secondary.
**Sources:** Federation §6.1.
**Rationale:** Visibility is the 80% use case. Read-only avoids distributed consistency problems (no merge conflicts, no consensus, no CRDTs needed across graphs). Each graph remains authoritative for its own state.
**Consequence:** Cross-repo task dispatch and dependencies are v0.2-v0.3 features, not v0.1.

### ADR-S6: The TUI IS the Interface

**Decision:** No platform-specific frontends. All platforms connect to the same TUI via terminal wrapping.
**Sources:** Terminal Wrapping §Core Principle.
**Rationale:** One codebase. Feature parity by default. Investment in the TUI benefits all platforms. Custom apps are high maintenance for marginal UX improvement.
**Consequence:** Mobile UX depends on responsive TUI (v0.3). Until then, mobile is "works but cramped." Custom apps are deferred until user demand warrants maintenance burden.

---

## 11. Open Questions

These require answers during implementation, not during design:

1. **Global `max_total_agents` cap:** Should there be a hard ceiling across all coordinators? The TUI concurrency design proposes this but doesn't specify the mechanism. Likely: sum of per-coordinator budgets, enforced in the coordinator tick loop.

2. **Chat conflict with multi-user:** Two users sending chat messages to the same coordinator simultaneously. The TUI concurrency design identifies this needs flock; the implementation detail is whether to flock the entire inbox file or use append-only with `O_APPEND`.

3. **Responsive TUI breakpoint thresholds:** Terminal wrapping suggests <50, 50-80, >80 cols. Need to validate against real Termux/Blink screen sizes during implementation.

4. **Event bus backpressure:** If a TUI subscriber is slow (e.g., on a mobile connection), the daemon's broadcast channel fills up. Strategy: bounded channel with drop-oldest semantics. Slow subscribers miss events and fall back to full reload on next fs watcher tick.

5. **Federation snapshot size:** For large graphs (1000+ tasks), even filtered `PeerGraphSnapshot` could be large. Consider pagination or summary-only mode (counts without task list) for the TUI Peers list view.

---

## References

| Resource | Location |
|----------|----------|
| TUI multiplexing design | `docs/design/tui-multiplexing-concurrent-access.md` |
| Terminal wrapping design | `docs/design/terminal-wrapping-strategy.md` |
| Federation architecture | `docs/design/federation-architecture.md` |
| Live sync & liveness design | `docs/design/live-sync-and-liveness.md` |
| Multi-user research | `docs/research/multi-user-tui-feasibility.md` |
| File locking audit | `docs/research/file-locking-audit.md` |
| Cross-repo communication | `docs/design/cross-repo-communication.md` |
| `modify_graph()` implementation | `src/parser.rs:267-283` |
| TUI fs watcher + refresh | `src/tui/viz_viewer/state.rs:3854-4234` |
| IPC protocol | `src/commands/service/ipc.rs` |
| Federation implementation | `src/federation.rs` |
| Peer commands | `src/commands/peer.rs` |
| Notification router | `src/notify/mod.rs` |
| Provenance system | `src/provenance.rs` |
| Task visibility field | `src/graph.rs:303` |
| Coordinator config | `src/config.rs:1794-1895` |
