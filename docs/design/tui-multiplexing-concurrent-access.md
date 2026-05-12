# Design: TUI Multiplexing & Concurrent Access Model

**Task:** mu-design-tui-concurrency
**Date:** 2026-03-25
**Author:** Architect agent
**Status:** Design document — input for mu-design-synthesis

---

## 1. Architecture Overview

```
┌──────────────────────────────────────────────────────────────────┐
│                        Shared VPS / Server                       │
│                                                                  │
│  ┌─────────────┐  ┌─────────────┐         ┌─────────────┐       │
│  │  User A      │  │  User B      │  ...   │  User N      │      │
│  │  SSH/tmux    │  │  SSH/tmux    │         │  ttyd/browser│      │
│  │  ┌─────────┐ │  │  ┌─────────┐ │         │  ┌─────────┐ │     │
│  │  │ wg tui  │ │  │  │ wg tui  │ │         │  │ wg tui  │ │     │
│  │  │ (pid A) │ │  │  │ (pid B) │ │         │  │ (pid N) │ │     │
│  │  └────┬────┘ │  │  └────┬────┘ │         │  └────┬────┘ │     │
│  └───────┼──────┘  └───────┼──────┘         └───────┼──────┘     │
│          │                 │                         │            │
│          ▼                 ▼                         ▼            │
│  ┌───────────────────────────────────────────────────────────┐   │
│  │              .wg/graph.jsonl                        │   │
│  │              (flock-serialized writes, atomic renames)     │   │
│  └───────────────────────────────────────────────────────────┘   │
│          │                 │                         │            │
│          ▼                 ▼                         ▼            │
│  ┌──────────────┐  ┌──────────────┐                              │
│  │ Coordinator A │  │ Coordinator B │  (per-user, namespaced)    │
│  │ (agents ≤ 4)  │  │ (agents ≤ 4)  │                            │
│  └──────────────┘  └──────────────┘                              │
│          │                 │                                      │
│          ▼                 ▼                                      │
│  ┌───────────────────────────────────────────────────────────┐   │
│  │                  Agent Pool (shared)                       │   │
│  │     flock-serialized graph access, worktree isolation      │   │
│  └───────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────┘
```

**Core principle:** Independent TUI sessions, shared graph state. Each user runs their own `wg tui` process. They don't see each other's cursor — they see each other's *effects* reflected in the shared graph within ~50ms.

---

## 2. TUI Instance Synchronization

### 2.1 Current Mechanism (Sufficient for Phase 1)

Each TUI instance already has a two-tier refresh model:

| Tier | Mechanism | Latency | Source |
|------|-----------|---------|--------|
| Fast | `notify` fs watcher on `.wg/` (recursive) | ~50ms (debounce) | `state.rs:3857-3883` |
| Slow | Periodic mtime comparison (1s tick) | ~1000ms | `state.rs:4086-4234` |

When any TUI or CLI command writes to `graph.jsonl` (via `modify_graph()`), the atomic rename triggers an inotify `IN_MOVED_TO` event. Every other TUI instance's fs watcher fires within 50ms, sets `fs_change_pending`, and the next event loop iteration performs a targeted reload.

**Verdict:** The existing fs watcher provides sub-100ms propagation for the shared-VPS scenario. No new transport is needed for Phase 1.

### 2.2 Enhanced Notification (Phase 2 — Optional)

For web clients (ttyd) or when inotify is unavailable:

```
┌──────────┐    IPC: GraphChanged    ┌──────────┐
│  wg tui  │ ◄─────────────────────► │  daemon   │
│  (user)  │    Unix socket          │  service  │
└──────────┘                         └──────────┘
```

The daemon already receives `GraphChanged` IPC events from CLI commands. Extend it to **broadcast** these to connected TUI instances:

1. TUI registers with daemon on startup: `{"cmd":"subscribe","client":"tui","pid":12345}`
2. Daemon maintains a subscriber list (Vec of Unix socket paths or connected streams)
3. On `GraphChanged`, daemon pushes `{"event":"graph_changed","mtime":"..."}` to all subscribers
4. TUI receives push → immediate reload (bypasses fs watcher latency)

**Latency improvement:** ~50ms (fs watcher debounce) → ~5ms (IPC push). Only worth building if the 50ms latency proves insufficient.

### 2.3 Performance: N Concurrent Users

| Users | Writes/sec (est.) | Read contention | FS events/sec | Impact |
|-------|-------------------|-----------------|---------------|--------|
| 2-3 | ~5-10 | None (shared locks) | ~10-20 | Unnoticeable |
| 5-7 | ~15-30 | None | ~30-60 | Minimal — each TUI reloads ~30 times/sec worst case |
| 10+ | ~40+ | None (reads never block) | ~80+ | Write serialization starts to matter; see §6 |

The bottleneck is **write throughput**, not read or notification. Each `modify_graph()` call takes ~10-50ms (read + modify + fsync + rename). At 10+ concurrent writers, lock contention adds queuing delay. For ≤7 users, the current model is adequate.

---

## 3. Conflict Resolution Strategy

### 3.1 Write Serialization via `modify_graph()`

The primary conflict prevention mechanism is `modify_graph()` (`parser.rs:267-283`):

```rust
pub fn modify_graph<P, F>(path: P, f: F) -> Result<wg, ParseError>
where F: FnOnce(&mut wg) -> bool {
    let _lock = FileLock::acquire(&lock_path)?;  // exclusive flock
    let mut graph = load_graph_inner(path)?;      // read under lock
    let modified = f(&mut graph);                 // modify under lock
    if modified { save_graph_inner(&graph, path)?; } // write under lock
    Ok(graph)
}
```

This holds the flock across the entire read-modify-write cycle, preventing TOCTOU races. Two users clicking "complete task" simultaneously are serialized: one's write completes, the other's closure sees the updated state.

### 3.2 Conflict Scenarios and Resolutions

| Scenario | Resolution | Mechanism |
|----------|-----------|-----------|
| Two users complete different tasks | Both succeed (serialized) | `modify_graph()` — closures run sequentially |
| Two users complete the same task | First wins, second is no-op | Closure checks status; already-done → return false |
| Two users edit same task description | Last writer wins | No character-level merging; this is acceptable |
| User edits while agent writes log | Both succeed | Different fields on the same task node |
| Two coordinators spawn for same task | First claim wins | `modify_graph()` sets status atomically; second sees `in-progress` |
| CLI command + TUI mutation | Serialized | Both use `modify_graph()` |

### 3.3 TOCTOU Migration Status

The file-locking audit (`docs/research/file-locking-audit.md`) identified commands still using the old `load_graph()` + `save_graph()` pattern. **All mutation commands MUST migrate to `modify_graph()` before multi-user deployment.** This is a prerequisite, not a nice-to-have.

**Action item:** Complete the `modify_graph()` migration for all remaining mutation commands (tracked separately).

### 3.4 Unsupported Conflict: Concurrent Text Editing

Real-time collaborative editing of the same task description (Google Docs-style) is **explicitly out of scope**. The graph is a task coordination tool, not a collaborative editor. If two users need to co-author a description, they should use external tools (shared doc, PR) and one user updates the task.

---

## 4. Per-User State Management

### 4.1 What State Is Per-User

Each TUI instance maintains **ephemeral view state** that is independent of the graph:

| State | Current Storage | Multi-User Impact |
|-------|----------------|-------------------|
| Selected task (cursor position) | In-memory (`VizApp`) | Independent per instance — no conflict |
| Right panel tab selection | In-memory | Independent |
| Scroll positions (graph, HUD, logs) | In-memory | Independent |
| Search query & results | In-memory | Independent |
| Panel focus (graph vs. panel) | In-memory | Independent |
| Chat history (per coordinator) | `.wg/chat/{id}/` | Shared — all users see same chat; see §4.3 |
| Active coordinator ID | In-memory | Independent selection, shared coordinators; see §5 |
| Mouse/input mode | In-memory | Independent |
| Filter/sort preferences | In-memory | Independent |
| Message drafts | In-memory | Independent (lost on exit) |
| File browser position | In-memory | Independent |

### 4.2 Design Decision: Ephemeral-Only View State

**Recommendation: Do NOT persist per-user view state to disk.**

Rationale:
- View state is cheap to reconstruct (user re-selects their task in <1 second)
- Persisting introduces file conflicts (whose `.tui-state.json` wins?)
- tmux already preserves session state across disconnects — the TUI stays running
- Complexity of user-namespaced state files outweighs the benefit

If persistence is ever needed (e.g., "remember my last-viewed task across TUI restarts"), use `~/.config/wg/tui-state.json` in the **user's home directory**, not in `.wg/`. This naturally namespaces by Unix user.

### 4.3 Shared State: Chat

Chat messages live in `.wg/chat/{coordinator_id}/inbox.jsonl` and `outbox.jsonl`. These are **shared across all TUI instances viewing the same coordinator**. This is the correct behavior — chat is a coordination channel, not a private view.

To attribute chat messages to users, add a `user` field to chat messages:

```json
{"id": 5, "text": "deploy the fix", "user": "alice", "timestamp": "2026-03-25T20:00:00Z"}
```

The `user` field comes from the `WG_USER` environment variable (see §5.2).

---

## 5. Multi-Coordinator Coexistence

### 5.1 Current Model

The TUI already supports multiple coordinators via a tab bar (`state.rs:2197`, `render.rs:1896`). Each coordinator:
- Has a numeric ID (0, 1, 2, ...)
- Runs as a separate process (Claude CLI or native LLM session)
- Manages its own chat inbox/outbox at `.wg/chat/{id}/`
- Shares a single `coordinator-state.json` (tracks ticks, agents, pause state)

The `CoordinatorConfig.max_coordinators` field (default: 16) limits concurrent coordinator processes.

### 5.2 Multi-User Coordinator Model

**Design: Each user gets their own coordinator, identified by `WG_USER`.**

```
User alice → coordinator 0 (label: "alice")
User bob   → coordinator 1 (label: "bob")
```

Implementation:
1. **User identity via `WG_USER`:** Each user sets `export WG_USER=alice` in their shell profile. This is propagated to all `wg` commands and logged in provenance.
2. **Coordinator auto-creation:** When a TUI starts, if no coordinator is labeled with the current `WG_USER`, create one automatically. The TUI already has `create_coordinator()` — extend it to set a label from `WG_USER`.
3. **Coordinator state namespacing:** Change `coordinator-state.json` from a single object to a map keyed by coordinator ID:

```json
{
  "0": {"enabled": true, "max_agents": 4, "executor": "claude", "label": "alice", ...},
  "1": {"enabled": true, "max_agents": 2, "executor": "claude", "label": "bob", ...}
}
```

4. **Agent budget isolation:** Each coordinator has its own `max_agents` budget. Alice's coordinator can run 4 agents; Bob's can run 4 agents. Total agents = sum of all coordinators' budgets (capped by a global `max_total_agents` config).

### 5.3 Coordinator Conflict Prevention

The key risk is two coordinators making conflicting dispatch decisions — e.g., both try to spawn an agent for the same ready task.

**Solution: Atomic task claiming via `modify_graph()`.**

The coordinator's dispatch loop already uses `modify_graph()` to set a task to `in-progress` and record the assigned agent. Because `modify_graph()` serializes writes:

1. Coordinator A's tick: sees task X as `open` + ready → `modify_graph()` sets X to `in-progress`, spawns agent
2. Coordinator B's tick (moments later): `modify_graph()` loads graph → sees X already `in-progress` → skips it

No additional locking needed. The existing `modify_graph()` serialization is sufficient.

**Edge case — settling delay:** The `settling_delay_ms` (default 2000ms) means coordinators batch their ticks. Two coordinators with different settling windows may both "see" a task as ready in their snapshot, but only one will win the `modify_graph()` race. The loser's closure returns `false` (no modification) and the task is simply skipped — no harm done.

### 5.4 Shared Visibility

All coordinators operate on the same graph. Every user's TUI sees all tasks, all agents, all coordinators. The coordinator tab bar shows all active coordinators with their labels:

```
[ alice (3 agents) ][ bob (1 agent) ][ + ]
```

Users can view any coordinator's chat, but the `WG_USER`-labeled coordinator is their "home" tab.

---

## 6. Flock Model: Required Changes

### 6.1 Current Flock Model Assessment

| Aspect | Current State | Multi-User Adequacy |
|--------|--------------|-------------------|
| `modify_graph()` (exclusive flock across RMW) | Implemented | **Sufficient** for ≤10 users |
| `load_graph()` (non-blocking shared lock) | Implemented | **Sufficient** — reads never block |
| Atomic write-rename | Implemented | **Sufficient** — readers always see consistent state |
| TOCTOU-safe commands | Partial (some migrated) | **Must complete** migration |
| Agent registry locking | Partial (`load_locked()` exists, not always used) | **Must audit** — multiple coordinators spawning concurrently |
| Provenance log | Append-only, no flock | **Sufficient** — `O_APPEND` atomic for <4KB writes |
| Chat files | No locking | **Needs flock** — multiple users may append to same inbox |

### 6.2 Required Changes

1. **Complete `modify_graph()` migration** — All mutation commands must use `modify_graph()`. (Priority: P0, prerequisite)

2. **Agent registry: use `load_locked()` universally** — With multiple coordinators spawning agents, the registry must be flock-protected for all read-modify-write operations, not just cleanup. (Priority: P0)

3. **Chat inbox flock** — When multiple users send chat messages to the same coordinator, the inbox append must be flock-protected. Use the same pattern: exclusive flock on `chat/{id}/inbox.lock` around the append. (Priority: P1)

4. **Coordinator state: per-coordinator files** — Replace single `coordinator-state.json` with `coordinator-state-{id}.json` to eliminate write contention between coordinators updating their own state. (Priority: P1)

### 6.3 Changes NOT Needed

- **No need for reader-writer locks** — The current exclusive-write / non-blocking-read model is correct. Readers see atomic snapshots via rename.
- **No need for database** — The single-file model scales to ~10 concurrent writers, which covers the target scenario.
- **No need for CRDT** — Single-machine, single-filesystem. Flock serialization is the right tool.
- **No need for IPC-serialized mutations** — Only needed at 10+ concurrent writers; defer.

---

## 7. User Identity Model

### 7.1 Mechanism

```bash
# In each user's ~/.bashrc or ~/.zshrc:
export WG_USER="alice"
```

All `wg` commands read `WG_USER` and include it in:
- **Provenance log entries** (`operations.jsonl`): `{"op":"set_status","task":"foo","user":"alice",...}`
- **Graph log entries**: `{"user":"alice","message":"Started working on..."}`
- **Chat messages**: `{"user":"alice","text":"deploy the fix"}`
- **Agent spawns**: Agent registry records which user's coordinator spawned the agent

### 7.2 Fallback

If `WG_USER` is not set, fall back to:
1. `$USER` (Unix username) — always available on shared VPS
2. `"unknown"` — last resort

### 7.3 What User Identity Enables

- **Audit trail**: "Who completed this task?" is answerable from provenance
- **Chat attribution**: Messages show sender name
- **Coordinator ownership**: Each coordinator is labeled with its user
- **TUI presence indicator** (future): "alice is viewing task X" — requires active heartbeat to a presence file

---

## 8. Summary: What Changes, What Doesn't

### No Changes Needed (Works Today)

- Multiple TUI instances on the same `.wg/` ✓
- FS watcher propagation between instances (~50ms) ✓
- `modify_graph()` serialization for migrated commands ✓
- Per-instance view state (cursor, scroll, tabs) ✓
- Multiple coordinators via TUI tab bar ✓
- Atomic read consistency (rename) ✓

### Required Changes (P0 — Before Multi-User)

| Change | Effort | Files |
|--------|--------|-------|
| Complete `modify_graph()` migration for all mutation commands | Medium | ~10 command files |
| Agent registry: universal `load_locked()` | Small | `service/spawn.rs` |
| `WG_USER` env var support in provenance + logs | Small | `provenance.rs`, `graph.rs`, log commands |

### Recommended Changes (P1 — Quality of Life)

| Change | Effort | Files |
|--------|--------|-------|
| Coordinator state: per-ID files | Small | `service/mod.rs` |
| Chat inbox flock protection | Small | `chat.rs` |
| Coordinator auto-creation from `WG_USER` | Small | `tui/viz_viewer/state.rs` |
| Chat message `user` field | Small | `chat.rs`, TUI render |

### Deferred (Phase 2+)

| Change | Trigger |
|--------|---------|
| IPC broadcast to TUI instances | If 50ms fs watcher latency is insufficient |
| TUI presence indicators | If users want to see who's online |
| Global `max_total_agents` cap | If coordinator agent budgets need hard ceiling |
| Responsive TUI for mobile screens | If mobile use grows |

---

## 9. Architecture Decision Records

### ADR-1: Single Shared wg (not per-user)

**Decision:** All users operate on one `.wg/graph.jsonl`.
**Rationale:** Simplest model, already works, matches "shared workspace" vision. Per-user workgraphs would require federation for basic task visibility.
**Consequence:** Conflict resolution is via flock serialization. No access control (all users can modify all tasks).

### ADR-2: Ephemeral View State (not persisted)

**Decision:** TUI view state (cursor, scroll, tabs) lives only in memory.
**Rationale:** tmux preserves session state. Persisting introduces namespacing complexity. Reconstruction cost is negligible.
**Consequence:** Restarting `wg tui` (without tmux) loses view position. Acceptable.

### ADR-3: `WG_USER` for Identity (not Unix UIDs)

**Decision:** User identity is an env var string, not a system UID.
**Rationale:** Flexible (works for containers, CI, remote), human-readable, zero system-level dependencies. Unix UIDs would require `getpwuid()` and don't work in all deployment models.
**Consequence:** Identity is advisory, not enforced. Users can set any `WG_USER`. This is fine — the system is collaborative, not adversarial.

### ADR-4: Coordinator-per-User with Shared Graph

**Decision:** Each user gets their own coordinator instance operating on the shared graph.
**Rationale:** Coordinators manage agent spawning. Per-user coordinators let each user control their own agent budget and chat independently.
**Consequence:** Multiple coordinators may race to claim tasks. `modify_graph()` serialization handles this correctly.

### ADR-5: No CRDT / No Database (for Phase 1)

**Decision:** Keep single-file JSONL with flock serialization.
**Rationale:** The target is ≤7 concurrent users on a single machine. Flock serialization provides linearizable writes at ~20-50 writes/sec — well within budget. CRDT or database adds complexity with no benefit at this scale.
**Consequence:** Scaling beyond ~10 writers requires architectural change (IPC-serialized mutations or database). This is explicitly deferred.

---

## References

| Resource | Location |
|----------|----------|
| Multi-user research | `docs/research/multi-user-tui-feasibility.md` |
| File locking audit | `docs/research/file-locking-audit.md` |
| Cross-repo communication | `docs/design/cross-repo-communication.md` |
| TUI fs watcher + refresh | `src/tui/viz_viewer/state.rs:3854-4234` |
| `modify_graph()` implementation | `src/parser.rs:267-283` |
| Screen dump IPC | `src/tui/viz_viewer/screen_dump.rs` |
| Coordinator config | `src/config.rs:1794-1895` |
| Coordinator state | `src/commands/service/mod.rs:308-346` |
| Agent registry locking | `src/commands/service/ipc.rs:621-669` |
