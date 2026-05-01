# Server-Side Multi-User Infrastructure: Implementation Work Plan

**Task:** mu-plan-server
**Date:** 2026-03-25
**Source:** [Unified Multi-User Platform Architecture](../design/multi-user-platform-architecture.md)
**Scope:** Server-side infrastructure for v0.1 (Multi-User Foundation MVP)

---

## Overview

This plan covers the server-side implementation required to support 2-7 concurrent users on a shared VPS, each running their own `wg tui`, managing their own coordinators, and seeing each other's effects in <100ms via the existing fs watcher.

**Total estimated effort:** 12-16 task-agent units (each unit ≈ one focused agent session)

---

## Task Inventory

### Task 1: Complete `modify_graph()` Migration — Phase 1 (Core Commands)

**Complexity:** L (Large)
**Priority:** P0 — hard prerequisite for everything else

**Description:**
The file locking audit (`docs/research/file-locking-audit.md`) identified a critical TOCTOU race: all graph-mutating commands use a `load_graph()` → modify → `save_graph()` pattern where the lock is NOT held across the full read-modify-write cycle. Under concurrent access, writes silently overwrite each other.

Currently only 8 files use `modify_graph()` (done, fail, eval_scaffold, coordinator, pause, resume, lib, parser). **76 files** still call `save_graph()`. Not all are production mutation paths — many are test helpers — but roughly 30-40 production command files need migration.

This task covers the highest-traffic mutation paths first:
- `src/commands/log.rs` — agents call `wg log` constantly
- `src/commands/artifact.rs` — agents register artifacts frequently
- `src/commands/claim.rs` — concurrent agent task claims
- `src/commands/add.rs` — concurrent task creation
- `src/commands/edit.rs` — task description edits
- `src/commands/link.rs` — dependency modifications
- `src/commands/abandon.rs` — task abandonment
- `src/commands/reject.rs`, `approve.rs` — validation workflow
- `src/commands/assign.rs` — agent assignment
- `src/commands/msg.rs` — message sending

**Files affected:** ~15 command files in `src/commands/`

**Dependencies:** None (foundational)

**Test strategy:**
- Unit test: `modify_graph()` closure correctly applies each mutation type
- Integration test: Two concurrent mutations (e.g., `wg log` from two processes) don't lose updates
- Stress test: 5 parallel agents doing `wg log` — verify no lost entries after 100 writes

**Risk factors:**
- Each command has unique mutation logic; no one-size-fits-all conversion
- Some commands do multiple loads/saves (e.g., `exec.rs` with 8 call sites) — need to understand whether they should be a single `modify_graph()` or multiple
- Test files also use `save_graph()` for setup — those can stay, but distinguishing test vs prod usage requires attention

---

### Task 2: Complete `modify_graph()` Migration — Phase 2 (Remaining Commands)

**Complexity:** L (Large)
**Priority:** P0

**Description:**
Migrate the remaining production `save_graph()` call sites:
- `src/commands/exec.rs` (8 call sites — complex, handles shell execution lifecycle)
- `src/commands/status.rs` (10 occurrences — batch status changes)
- `src/commands/agent.rs` (11 occurrences — agent CRUD lifecycle)
- `src/commands/service/ipc.rs` (9 occurrences — daemon-side mutations)
- `src/commands/service/coordinator.rs` (already partially migrated, verify completeness)
- `src/commands/service/triage.rs`, `service/coordinator_agent.rs`
- `src/commands/spawn/execution.rs`, `spawn/mod.rs`
- `src/tui/viz_viewer/event.rs`, `state.rs` (TUI-initiated mutations)
- `src/executor/native/tools/wg.rs` (native executor tool calls)
- `src/federation.rs` (federation-initiated writes)
- `src/matrix_commands.rs` (Matrix bot mutations)
- All remaining: sweep, gc, reclaim, retry, reschedule, checkpoint, plan, coordinate, etc.

**Files affected:** ~35 files across `src/commands/`, `src/tui/`, `src/executor/`, `src/federation.rs`

**Dependencies:** Task 1 (establishes the pattern and catches early issues)

**Test strategy:**
- Each migrated command gets a test verifying idempotent behavior under `modify_graph()`
- Full integration: run `cargo test` — all existing tests must pass
- Concurrent stress test: extended version of Task 1's stress test covering all mutation paths

**Risk factors:**
- IPC handler (`service/ipc.rs`) mutations happen inside the daemon process — need to verify the flock doesn't deadlock with the daemon's own graph access
- TUI mutations (event.rs) happen in an async context — `modify_graph()` must not block the event loop unacceptably
- Some commands may need `modify_graph()` to return additional data beyond the graph (e.g., the ID of a newly created task) — may need a `modify_graph_with_result()` variant

---

### Task 3: Agent Registry Universal `load_locked()`

**Complexity:** S (Small)
**Priority:** P0

**Description:**
The agent registry (`src/service/registry.rs`) has both locked (`load_locked()`) and unlocked (`load()`/`save()`) access patterns. Under multiple coordinators spawning agents concurrently, unlocked access races. Migrate all registry write paths to use `load_locked()`.

Currently `load_locked()` is used in 11 files. The unlocked `load()`/`save()` pattern must be found and migrated in:
- `src/commands/spawn/execution.rs` — agent spawn registration
- `src/commands/spawn/mod.rs` — spawn orchestration
- Any other registry writers

**Files affected:** `src/service/registry.rs`, `src/commands/spawn/execution.rs`, `src/commands/spawn/mod.rs`

**Dependencies:** None (independent of modify_graph migration)

**Test strategy:**
- Test: two concurrent `LockedRegistry` acquisitions from separate threads — second blocks until first is dropped
- Test: spawn two agents concurrently — both appear in registry
- Verify existing `dead_agents`, `kill`, `heartbeat` tests still pass

**Risk factors:**
- Low risk. The `load_locked()` pattern already exists and is proven. This is extending its use.
- Potential deadlock if a codepath holds the registry lock while also acquiring the graph lock — audit for nested locks

---

### Task 4: `WG_USER` Identity System

**Complexity:** M (Medium)
**Priority:** P0

**Description:**
Implement the `WG_USER` environment variable support throughout the system. The fallback chain is: `WG_USER` → `$USER` → `"unknown"`.

Create a shared utility function (e.g., `fn current_user() -> String` in `src/lib.rs` or a new `src/identity.rs`) that encapsulates this fallback logic.

Wire `WG_USER` into:
1. **Provenance log** (`src/provenance.rs`): Add `user` field to provenance entries
2. **Task logs** (`src/commands/log.rs`): Include user in log entry metadata
3. **Chat messages** (`src/chat.rs`): Add `user` field to chat messages, display in TUI
4. **Coordinator labels** (`src/commands/service/coordinator.rs`): Auto-label coordinators with creating user
5. **Graph mutations** (via `modify_graph()`): Record mutating user in provenance

**Files affected:** `src/lib.rs` (or new `src/identity.rs`), `src/provenance.rs`, `src/chat.rs`, `src/commands/log.rs`, `src/commands/service/coordinator.rs`, `src/graph.rs` (if Task struct gains optional user fields)

**Dependencies:** None (independent, but best done after Task 1 so provenance writes use `modify_graph()`)

**Test strategy:**
- Unit test: `current_user()` returns `WG_USER` when set, `$USER` when not, `"unknown"` when neither
- Unit test: provenance entry includes `user` field
- Unit test: chat message serialization includes `user` field
- Integration test: set `WG_USER=alice`, run `wg log`, verify log entry attributes to alice

**Risk factors:**
- Schema evolution: adding `user` field to provenance/chat JSON must be backward-compatible (missing field → None/default)
- The `WG_USER` env var must propagate correctly to spawned agent processes — verify the executor passes it through

---

### Task 5: Per-User Coordinator Creation

**Complexity:** S (Small)
**Priority:** P0

**Description:**
When a user launches `wg tui`, auto-create a coordinator labeled with their `WG_USER` identity. This enables each user to have their own coordinator managing their own agent budget.

Currently coordinators are created via the TUI "+" key or `wg service start`. The change:
- On TUI startup, if no coordinator exists for the current `WG_USER`, prompt or auto-create one
- Coordinator state files become per-ID: `coordinator-state-{id}.json` instead of shared `coordinator-state.json`
- The service daemon manages multiple coordinators, each with independent agent budgets

**Files affected:** `src/tui/viz_viewer/state.rs` (TUI startup), `src/commands/service/coordinator.rs` (coordinator lifecycle), `src/commands/service/mod.rs` (state file paths)

**Dependencies:** Task 4 (`WG_USER` must exist)

**Test strategy:**
- Test: TUI startup with `WG_USER=alice` creates coordinator labeled "alice"
- Test: Two coordinators (alice, bob) can run simultaneously without state file conflicts
- Test: Per-ID state files are correctly read/written

**Risk factors:**
- Backward compatibility: existing single `coordinator-state.json` must be migrated or handled as fallback
- The TUI tab bar already supports multiple coordinators — verify the auto-creation integrates cleanly

---

### Task 6: Per-Coordinator State Files

**Complexity:** S (Small)
**Priority:** P1

**Description:**
Split `coordinator-state.json` into per-coordinator files (`coordinator-state-{id}.json`) to eliminate write contention between coordinators updating their own state.

Currently the coordinator state (last tick time, agent counts, etc.) is stored in a single shared file. With multiple coordinators, this is a write contention point.

**Files affected:** `src/commands/service/coordinator.rs`, `src/commands/service/mod.rs`, any code reading coordinator state

**Dependencies:** Task 5 (per-user coordinators must exist first)

**Test strategy:**
- Test: two coordinators write state simultaneously without corruption
- Test: `wg service status` reads all per-coordinator state files correctly
- Test: migration from old single file to per-ID files

**Risk factors:**
- Low risk. Straightforward file path change.
- Need to handle the migration path: first run after upgrade should convert old file to new format

---

### Task 7: Chat Inbox Flock Protection

**Complexity:** S (Small)
**Priority:** P1

**Description:**
Add flock-based locking to chat inbox file operations (`src/chat.rs`). Two users sending messages to the same coordinator simultaneously can race on the inbox file.

Options identified in the architecture doc:
- flock the entire inbox file (simpler, sufficient for ≤7 users)
- Use `O_APPEND` for append-only semantics (similar to provenance log)

Recommendation: Use flock (consistent with the rest of the system). The inbox files are small and contention is low.

**Files affected:** `src/chat.rs`

**Dependencies:** None (independent)

**Test strategy:**
- Test: two concurrent chat message sends don't lose either message
- Test: reading inbox while another process writes doesn't see partial data

**Risk factors:**
- Very low risk. Standard flock pattern already well-established in codebase.

---

### Task 8: Chat Message `user` Field

**Complexity:** S (Small)
**Priority:** P1

**Description:**
Add a `user` field to chat message structs, populated from `current_user()`. Display the user attribution in the TUI chat panel.

**Files affected:** `src/chat.rs` (message struct), TUI chat rendering (likely in `src/tui/viz_viewer/render.rs`)

**Dependencies:** Task 4 (`current_user()` utility)

**Test strategy:**
- Test: chat message serialization roundtrips with user field
- Test: backward compat — old messages without user field deserialize to `user: None`
- Visual test: TUI displays "alice: message" format

**Risk factors:**
- Low risk. Additive schema change with serde defaults.

---

### Task 9: fs Watcher Validation for Multi-User

**Complexity:** S (Small)
**Priority:** P1

**Description:**
Validate that the existing fs watcher (50ms debounce, inotify-based) works correctly with multiple concurrent TUI instances. The architecture doc states this should work with <100ms propagation for ≤7 users.

This is a validation/hardening task, not new feature work:
- Verify multiple TUI instances all receive `IN_MOVED_TO` events from atomic renames
- Verify debounce doesn't cause missed updates under burst writes
- Verify inotify watch limit is sufficient (default is 8192, each TUI adds ~5 watches)
- Document any edge cases or tuning parameters

**Files affected:** `src/tui/viz_viewer/state.rs` (fs watcher setup), possibly `docs/` for operational docs

**Dependencies:** Tasks 1-2 (all mutations must use `modify_graph()` with atomic rename for consistent inotify events)

**Test strategy:**
- Manual test: 5 TUI instances, rapid `wg log` from CLI, verify all TUIs update within 100ms
- Automated test: spawn N file watchers, perform M atomic renames, verify all N receive all M events

**Risk factors:**
- Medium risk. inotify has known edge cases (moved files, renamed directories, watch limits). The existing implementation may already handle these, but multi-user amplifies any latency.

---

### Task 10: `wg server init` Automation

**Complexity:** M (Medium)
**Priority:** P1

**Description:**
Create a `wg server init` command that automates server setup for multi-user deployment. Based on the architecture doc's deployment steps:

1. Check/install prerequisites: tmux, ttyd (optional), caddy (optional)
2. Create Unix group for the project (e.g., `wg-<project>`)
3. Set directory permissions: `.wg/` owned by project group, 0770
4. Set file permissions: `graph.jsonl` 0660, `daemon.sock` 0660
5. Generate per-user shell profile snippet: `export WG_USER="<name>"`
6. Generate tmux launch command: `tmux new-session -A -s "${WG_USER}-wg" "wg tui"`
7. Optionally generate ttyd + Caddy config for web access
8. Print summary of what was configured

**Files affected:** New file `src/commands/server.rs`, `src/commands/mod.rs` (command registration), `src/cli.rs` (CLI definition)

**Dependencies:** Task 4 (`WG_USER` must be implemented)

**Test strategy:**
- Test: `wg server init --dry-run` prints expected commands without executing
- Test: validates prerequisites are installed
- Test: generated shell snippet sets correct env vars
- Manual test: run on fresh VPS, follow output, verify multi-user setup works

**Risk factors:**
- Medium risk. OS-level operations (group creation, permission changes) require appropriate privileges and vary by distribution
- Should use `--dry-run` by default and require `--apply` for actual changes
- Must handle both fresh installs and upgrades gracefully

---

### Task 11: tmux Session Management

**Complexity:** S (Small)
**Priority:** P1

**Description:**
Integrate tmux session management with workgraph:
- `wg server connect [user]` — creates or attaches to a user's tmux session (`${WG_USER}-wg`)
- Verify the TUI works correctly inside tmux (already should, but validate)
- Ensure `wg tui` detects it's inside tmux and adjusts behavior if needed (e.g., no nested tmux)

**Files affected:** New subcommand in `src/commands/server.rs`, or extend `src/commands/setup.rs`

**Dependencies:** Task 10 (`wg server init` establishes the tmux convention)

**Test strategy:**
- Test: `wg server connect` creates tmux session with correct name
- Test: re-running `wg server connect` reattaches to existing session
- Test: `WG_USER` is correctly propagated inside the tmux session

**Risk factors:**
- Low risk. tmux is a well-understood tool. The integration is primarily shell scripting.
- Edge case: what happens if tmux is not installed? Graceful error with install instructions.

---

### Task 12: Liveness Indicators (HUD)

**Complexity:** S (Small)
**Priority:** P1

**Description:**
Add basic liveness indicators to the TUI:
1. "Time since last graph event" counter in the HUD bar
2. Agent elapsed time displayed in graph view nodes (how long each agent has been working)

These use existing data (graph file mtime for events, agent registry timestamps for elapsed time) — no new infrastructure needed.

**Files affected:** `src/tui/viz_viewer/state.rs` (data computation), `src/tui/viz_viewer/render.rs` (display)

**Dependencies:** None (uses existing data)

**Test strategy:**
- Test: HUD shows correct elapsed time since last graph modification
- Test: agent elapsed time renders correctly for active agents
- Visual test: verify rendering looks reasonable in various terminal widths

**Risk factors:**
- Low risk. Pure display changes using existing data sources.

---

## Dependency Graph

```
                    ┌─────────┐
                    │ Task 1  │  modify_graph Phase 1
                    │  (L)    │
                    └────┬────┘
                         │
                    ┌────▼────┐
                    │ Task 2  │  modify_graph Phase 2
                    │  (L)    │
                    └────┬────┘
                         │
                    ┌────▼────┐
                    │ Task 9  │  fs watcher validation
                    │  (S)    │
                    └─────────┘

  ┌─────────┐       ┌─────────┐       ┌─────────┐
  │ Task 3  │       │ Task 4  │       │ Task 7  │
  │ Registry│       │ WG_USER │       │ Chat    │
  │ Locking │       │ Identity│       │ flock   │
  │  (S)    │       │  (M)    │       │  (S)    │
  └─────────┘       └────┬────┘       └─────────┘
                         │
             ┌───────────┼───────────┐
             │           │           │
        ┌────▼────┐ ┌────▼────┐ ┌────▼────┐
        │ Task 5  │ │ Task 8  │ │ Task 10 │
        │ Per-user│ │ Chat    │ │ Server  │
        │ Coord.  │ │ user    │ │ Init    │
        │  (S)    │ │  (S)    │ │  (M)    │
        └────┬────┘ └─────────┘ └────┬────┘
             │                       │
        ┌────▼────┐            ┌────▼────┐
        │ Task 6  │            │ Task 11 │
        │ Per-ID  │            │ tmux    │
        │ State   │            │ Mgmt    │
        │  (S)    │            │  (S)    │
        └─────────┘            └─────────┘

  ┌─────────┐
  │ Task 12 │  (independent)
  │ Liveness│
  │  (S)    │
  └─────────┘
```

---

## Critical Path

The critical path (longest sequential chain) determines the minimum elapsed time:

```
Task 1 (L) → Task 2 (L) → Task 9 (S)
```

**Critical path effort: ~5-7 task-agent units**

The `modify_graph()` migration is the bottleneck. It cannot be parallelized because all migrations touch the same core pattern and later migrations learn from earlier ones.

Everything else can run in parallel once Task 4 (`WG_USER`) is complete:
- Tasks 3, 7, 12 are fully independent
- Tasks 5, 8, 10 depend only on Task 4
- Tasks 6, 11 are short tails

**Optimal parallelism schedule:**

| Wave | Tasks | Agents | Duration |
|------|-------|--------|----------|
| Wave 1 | 1 (modify_graph P1), 3 (registry), 4 (WG_USER), 7 (chat flock), 12 (liveness) | 5 | 2-3 units |
| Wave 2 | 2 (modify_graph P2), 5 (per-user coord), 8 (chat user), 10 (server init) | 4 | 2-3 units |
| Wave 3 | 9 (fs watcher validation), 6 (per-ID state), 11 (tmux mgmt) | 3 | 1 unit |

**Total wall-clock estimate with 4-5 parallel agents: ~5-7 task-agent sessions**

---

## Summary Table

| # | Task | Complexity | Priority | Dependencies | Files Affected | Test Strategy |
|---|------|-----------|----------|--------------|----------------|---------------|
| 1 | modify_graph Migration P1 (core cmds) | L | P0 | None | ~15 cmd files | Concurrent write stress test |
| 2 | modify_graph Migration P2 (remaining) | L | P0 | Task 1 | ~35 files | Full cargo test + stress test |
| 3 | Registry universal load_locked() | S | P0 | None | 3 files | Concurrent spawn test |
| 4 | WG_USER identity system | M | P0 | None | 6 files | Env var fallback + integration |
| 5 | Per-user coordinator creation | S | P0 | Task 4 | 3 files | Multi-coordinator startup |
| 6 | Per-coordinator state files | S | P1 | Task 5 | 3 files | Concurrent state writes |
| 7 | Chat inbox flock protection | S | P1 | None | 1 file | Concurrent message test |
| 8 | Chat message user field | S | P1 | Task 4 | 2 files | Serde roundtrip + backcompat |
| 9 | fs watcher multi-user validation | S | P1 | Tasks 1-2 | 2 files | Multi-TUI propagation test |
| 10 | wg server init automation | M | P1 | Task 4 | 3 new files | Dry-run + manual VPS test |
| 11 | tmux session management | S | P1 | Task 10 | 1-2 files | Session create/attach |
| 12 | Liveness indicators (HUD) | S | P1 | None | 2 files | Visual + unit tests |

**Totals:** 12 tasks, 2L + 2M + 8S = **12-16 task-agent units**

---

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|-----------|
| modify_graph migration introduces regressions | Medium | High | Phased migration (P1 then P2), comprehensive test coverage, each migration is a separate commit |
| Nested flock deadlock (graph lock + registry lock) | Low | High | Audit all lock acquisition orders; document lock hierarchy (graph before registry, always) |
| WG_USER env var not propagated to agent subprocesses | Medium | Medium | Add to executor env setup; test with `env` inspection in spawned agent |
| inotify missed events under burst writes | Low | Medium | Debounce already exists (50ms); validation task will stress-test |
| Per-coordinator state migration breaks existing installs | Low | Medium | Fallback: read old single file if per-ID file not found |
| wg server init fails on non-standard Linux setups | Medium | Low | --dry-run default; document supported distros |
