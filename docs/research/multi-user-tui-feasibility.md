# Multi-User TUI Feasibility & Prior Art

**Task:** mu-research
**Date:** 2026-03-25
**Author:** Thorough Documenter agent

---

## Executive Summary

wg's architecture is surprisingly well-positioned for multi-user operation. The combination of file-based state (JSONL), flock-based locking, `modify_graph()` atomic transactions, filesystem watching, and existing federation/peer infrastructure means that multiple users SSH'ing into a shared server and each running their own TUI instance is **already largely functional today**. The main gaps are: (1) the TOCTOU race in commands that haven't migrated to `modify_graph()`, (2) no real-time cross-instance notification of graph changes beyond filesystem polling, and (3) federation is agency-only — no cross-wg task visibility yet.

---

## 1. Current TUI Concurrency

### 1.1 How the TUI Detects Changes

The TUI uses a **two-tier refresh model**:

- **Fast path (< 100ms):** A `notify` filesystem watcher (`notify_debouncer_mini`) monitors the entire `.wg/` directory recursively. When any file changes, it sets an `AtomicBool` flag (`fs_change_pending`). On the next event loop iteration, the TUI checks this flag and performs targeted reloads:
  - `graph.jsonl` mtime change → full viz reload + stats + agent monitor + HUD
  - `messages/{task-id}.jsonl` mtime change → messages panel reload
  - `service/daemon.log` mtime change → coordinator log reload
  - `chat/{id}/outbox.jsonl` mtime change → chat poll
  - Agent output files → stream updates

- **Slow path (1-second tick):** A periodic timer (`refresh_interval = 1s`) performs graph mtime comparison and reloads if changed. This is the fallback when the filesystem watcher is unavailable (e.g., on some NFS mounts).

**Source:** `src/tui/viz_viewer/state.rs:3854-4230` — `start_fs_watcher()` and `maybe_refresh()`

### 1.2 What Happens with Two TUI Instances

Two TUI instances on the same `.wg/` directory **work today**, with caveats:

- **Reads are safe.** `load_graph()` uses a non-blocking shared lock (`LOCK_SH | LOCK_NB`). If another process holds an exclusive lock, the read proceeds anyway — safe because `save_graph_inner()` uses atomic temp-file-rename, so readers always see a consistent snapshot (pre- or post-write, never partial).

- **Writes via TUI are safe IF they use `modify_graph()`.** The `modify_graph()` function (added to fix the TOCTOU race documented in `docs/research/file-locking-audit.md`) holds an exclusive flock across the entire load→modify→save transaction. Commands that have migrated to `modify_graph()` include: `done`, `fail`, `pause`, `resume`.

- **Writes via older commands may race.** Some commands may still use the separate `load_graph()` + `save_graph()` pattern. Under concurrent mutation, this can silently lose updates (the TOCTOU race).

- **Both instances see each other's changes** within ~50ms (fs watcher debounce) to ~1s (polling fallback). This provides a "live" feel where one user's actions (completing tasks, adding logs, spawning agents) are visible to the other almost immediately.

### 1.3 Screen Dump IPC

The TUI already exposes its rendered screen via a Unix domain socket at `.wg/service/tui.sock`. After each frame render, the screen buffer is serialized to plain text and stored in a `SharedScreen` (Arc<Mutex>). External clients can connect and read the current screen contents as structured JSON. This is designed for agents to observe the TUI, but could be repurposed for web-based screen mirroring.

**Source:** `src/tui/viz_viewer/screen_dump.rs`

### 1.4 Concurrency Verdict

| Scenario | Status | Risk |
|----------|--------|------|
| Two TUI instances, read-only browsing | **Works** | None |
| Two TUI instances, both mutating graph | **Mostly works** | Low — `modify_graph()` prevents races for migrated commands |
| TUI + CLI commands concurrently | **Mostly works** | Low — same as above |
| TUI + coordinator + agents | **Works** | Low — coordinator uses `modify_graph()` |
| Two coordinators on same graph | **Untested** | Medium — coordinator state files may conflict |

---

## 2. Terminal-Over-Web

### 2.1 Approaches Compared

| Tool | Architecture | Maturity | Latency | Authentication | Active Development |
|------|-------------|----------|---------|----------------|-------------------|
| **ttyd** | C binary, libwebsockets, xterm.js frontend | Production-ready | ~10-30ms | Basic auth, SSL/TLS, token-based | Yes (5k+ GitHub stars) |
| **wetty** | Node.js, socket.io, xterm.js frontend | Mature | ~30-50ms | Integrates with SSH auth | Slower, but stable |
| **gotty** | Go binary, WebSocket, hterm frontend | Mature but stale | ~10-30ms | Basic auth, SSL | Low activity since 2019 |
| **xterm.js direct** | JS library, custom backend needed | Component-only | Custom | Custom | Very active (core lib) |
| **Zellij session sharing** | Rust terminal multiplexer, built-in sharing | Emerging | ~10ms | Plugin-based | Yes |

### 2.2 Recommendation: ttyd

**ttyd** is the strongest candidate for exposing wg's TUI via browser:

- **Single binary, zero dependencies** — `ttyd -p 8080 wg tui` immediately works
- **Full xterm.js integration** — true terminal emulation, handles TUI rendering (ratatui), mouse events, bracketed paste, 256-color
- **WebSocket transport** — low latency, bidirectional
- **Authentication** — basic auth (`-c user:pass`), client certificate auth, and can be placed behind a reverse proxy (nginx, Caddy) for OAuth/SSO
- **Read-only mode** — `ttyd -R` exposes a view-only terminal, useful for dashboards
- **Per-client sessions** — each browser tab gets its own PTY, running its own `wg tui` instance (which is exactly the multi-user model)

**Deployment pattern:**
```
VPS
├── ttyd -p 8080 --credential user:pass tmux new-session -A -s $USER "wg tui"
│   ├── Browser User A → PTY → tmux session → wg tui instance
│   └── Browser User B → PTY → tmux session → wg tui instance
└── .wg/graph.jsonl  ← shared state, flock-protected
```

### 2.3 xterm.js Direct Integration (Future)

For a more integrated experience, a custom web frontend could:
1. Embed xterm.js as a component
2. Connect to a WebSocket backend (could be a small Rust server using `tokio-tungstenite`)
3. Spawn `wg tui` in a PTY per connection
4. Add web-native UI chrome around the terminal (user list, notifications, links)

This is more work but provides the most polished experience. The ttyd approach is a pragmatic first step.

### 2.4 Considerations

- **Terminal size negotiation:** xterm.js sends SIGWINCH on resize; ratatui handles this correctly. The TUI already adapts to arbitrary terminal sizes.
- **Mouse support:** The TUI already detects mosh and adjusts mouse modes (`set_mouse_capture()` in `event.rs:30-43`). Browser-based terminals generally support mode 1002/1006 well.
- **Clipboard:** xterm.js supports OSC 52 clipboard integration. The TUI's bracketed paste support (`event.rs:73-85`) works over WebSocket.
- **Latency:** For a VPS in the same region, expect 10-50ms round-trip. The TUI's adaptive poll timeout (`next_poll_timeout()`) keeps idle CPU low.

---

## 3. Mobile Terminal State of the Art

### 3.1 Android: Termux

| Aspect | Status |
|--------|--------|
| **SSH client** | Full OpenSSH, works well |
| **Mosh client** | Available via `pkg install mosh`, works well |
| **tmux** | Full support, `pkg install tmux` |
| **Terminal emulation** | 256-color, true-color (some devices), Unicode |
| **Touch interaction** | Volume keys as Ctrl/Alt modifiers, gesture support |
| **Screen size** | Typically 40-80 cols on phone, 100+ on tablet |
| **wg TUI** | **Already detected** — `detect_termux_touch()` in `event.rs:48-50` enables mode 1003 for touch drag events |
| **Background execution** | `termux-wake-lock` prevents Android from killing sessions |

**Key finding:** The TUI already has Termux-specific code. The `TERMUX_VERSION` env var detection and mosh-aware mouse mode switching (`event.rs:26-50`) show that mobile access has already been considered and partially implemented.

**Limitations:**
- Small screen real estate — the TUI's multi-panel layout may need responsive breakpoints
- On-screen keyboard obscures half the screen — keyboard shortcuts may need alternatives
- Android battery optimization can kill background SSH/mosh sessions
- No split-pane multitasking on most phones (works on tablets)

### 3.2 iOS: Blink Shell / a-Shell

| App | SSH | Mosh | tmux | True-color | Price |
|-----|-----|------|------|------------|-------|
| **Blink Shell** | Yes | Yes (built-in) | Yes (remote) | Yes | $15.99 |
| **a-Shell** | Yes | No | No (local only) | Limited | Free |
| **iSH** | Yes (Alpine Linux) | Possible but slow | Yes | Limited | Free |

**Blink Shell** is the gold standard for iOS terminal access:
- Native mosh implementation (not a port)
- Hardware keyboard support with full modifier keys
- Font customization, themes, snappy rendering
- Files app integration for SFTP

**Limitation:** iOS apps cannot run background processes indefinitely. Mosh's reconnection model handles this gracefully — the session persists on the server, and the client reconnects when the app returns to foreground.

### 3.3 Mobile Verdict

The **SSH/mosh → tmux → wg tui** stack works today on both platforms. The main UX challenges are:
1. **Screen size** — The TUI needs a minimum viable layout for ~40-column screens
2. **Input** — Touch-friendly navigation (the scrollbar dragging code in `event.rs` already handles this)
3. **Reconnection** — Mosh handles this natively; tmux preserves session state

---

## 4. Multi-User Terminal Sharing

### 4.1 Prior Art

| Tool | Model | Real-time? | Access Control | Status |
|------|-------|-----------|---------------|--------|
| **tmux shared sessions** | Multiple clients attach to same session | Yes — same screen | Unix permissions on socket | Built-in to tmux |
| **tmate** | tmux fork with relay server | Yes — same screen | Unique session URLs | Active, hosted relay |
| **wemux** | tmux wrapper, multi-mode | Yes — mirror/pair/rogue modes | User-based | Maintained |
| **Zellij sharing** | Built-in plugin system | Yes — per-pane | Plugin-controlled | In development |
| **VS Code Live Share** | Editor-specific, WebSocket | Yes | Microsoft auth | Mature |
| **Tuple** | Screen sharing + voice | Yes | Invite-based | Commercial |

### 4.2 wg's Model is Different

The prior art above focuses on **screen sharing** — multiple users seeing the same terminal output. wg's vision is fundamentally different: **independent sessions, shared state**.

Each user runs their own TUI instance, seeing the full graph from their own perspective. They don't need to see each other's cursor or terminal — they see each other's *effects* (task completions, log entries, agent spawns) reflected in the shared graph state.

This is architecturally simpler and more scalable than screen sharing:

| Screen Sharing Model | wg Model |
|---------------------|-----------------|
| N users share 1 terminal session | N users have N terminal sessions |
| Single cursor, turn-taking | Independent cursors, parallel work |
| Bandwidth scales with screen size | Bandwidth scales with mutation rate |
| Requires specialized relay/protocol | Uses existing filesystem + flock |
| Limited to terminal width/height | Each user has their own terminal size |

### 4.3 What's Still Useful from Prior Art

- **tmate's relay model** could inspire a lightweight notification relay: instead of relying solely on filesystem watching, a small WebSocket server could push "graph changed" events to all connected TUI instances, reducing poll latency to near-zero.
- **tmux's socket model** is already the deployment vehicle — each user gets a named tmux session.
- **wemux's "rogue mode"** (each user in their own pane but same tmux session) could be useful for pairing scenarios where users *want* to see each other's terminals.

---

## 5. Federation Model

### 5.1 Current State

Federation in WG currently covers **agency entities only** (roles, tradeoffs, agents, evaluations):

- **`src/federation.rs`** — Core transfer logic between agency stores. Content-addressed entities (SHA-256 IDs) make federation conflict-free.
- **`.wg/federation.yaml`** — Named remotes (agency stores) and named peers (other WG instances).
- **Commands:** `wg agency pull/push/scan/remote/merge` — all operational today.

The **peer system** (`wg peer add/remove/list/show/status`) is also implemented, providing named references to other WG instances with service status detection.

### 5.2 Cross-Repo Communication (Designed, Partially Implemented)

The design document at `docs/design/cross-repo-communication.md` describes:

- **`wg add --repo <peer>`** — Create a task in another WG instance (via IPC if service is running, direct file access otherwise)
- **Cross-repo dependencies** — `repo:task-id` syntax for references
- **`AddTask` and `QueryTask` IPC requests** — New IPC message types for remote task creation and status queries
- **Peer resolution** — Named peers → path → socket discovery → IPC or file fallback

### 5.3 What Cross-WG Visibility Would Look Like

For the multi-user scenario on a single VPS, the key insight is that **all WG instances share a filesystem**. This means:

1. **Same WG instance, multiple users** — No federation needed. All users operate on the same `.wg/graph.jsonl`. Each user has their own coordinator (multiple coordinators are already supported via the TUI's coordinator tab system). This is the simplest and most immediately viable model.

2. **Separate WG instances, same machine** — Use the existing peer system. Each project has its own `.wg/`. Users can `wg peer add` each other's WG instances. Cross-repo task dispatch via `wg add --repo` sends tasks between them.

3. **Separate machines** — Requires network-accessible IPC. The current Unix domain socket is local-only. Options:
   - SSH tunneling: `ssh -L local.sock:remote.sock server` — works today, manual setup
   - TCP IPC: Extend the daemon to listen on a TCP port — straightforward but needs authentication
   - Git-based sync: Push/pull the graph file via git — eventual consistency with merge

### 5.4 Federation Gaps for Multi-User

| Gap | Impact | Difficulty |
|-----|--------|-----------|
| No cross-wg task visibility in TUI | Users can't see peer tasks without switching | Medium — add a "peers" panel |
| Agency federation is manual (`pull`/`push`) | No automatic sharing of agent performance data | Low — could auto-sync on coordinator tick |
| No user identity in the graph | Can't attribute actions to specific users | Low — add `user` field to log entries |
| Coordinator-per-user isolation | Multiple coordinators may make conflicting decisions | Medium — need coordinator namespacing |

---

## 6. Synchronization Model

### 6.1 Current Model: Single-File, flock-Serialized

The graph lives in a single file (`.wg/graph.jsonl`). All mutations go through `modify_graph()` which holds an exclusive flock for the entire read-modify-write transaction. This provides:

- **Linearizability** for local mutations (no lost updates when using `modify_graph()`)
- **Crash safety** via atomic temp-file-rename
- **Read consistency** via atomic rename (readers see pre- or post-write, never partial)

### 6.2 Scaling Characteristics

| Metric | Current Performance | Multi-User Impact |
|--------|-------------------|-------------------|
| Graph size | ~100-500KB typical, up to several MB | Grows linearly with tasks; not a bottleneck |
| Write latency | ~10-50ms (read + modify + write + fsync) | Serialized under flock; contention under many concurrent writers |
| Read latency | ~5-20ms (no lock contention for shared reads) | Unchanged — non-blocking shared locks |
| Write throughput | ~20-50 writes/sec (limited by fsync) | Limited by single-writer serialization |
| Notification latency | ~50ms (fs watcher) to ~1s (polling) | Unchanged — each TUI instance has its own watcher |

For 2-5 users with their own coordinators and agents, the single-file model should work without modification. Write contention becomes noticeable at ~10+ concurrent writers.

### 6.3 Eventual Consistency via Git

The graph file is designed to be version-control-friendly (JSONL, one line per node). A git-based sync model would look like:

```
User A (laptop) ──push──> git remote <──pull── User B (server)
       └── .wg/graph.jsonl
```

**Challenges:**
- **Merge conflicts** — Two users modifying the same task's line creates a git conflict. JSONL's one-line-per-node format means conflicts are isolated to individual tasks, but resolution requires domain knowledge (which status wins? which log entries to keep?).
- **Ordering** — Git doesn't preserve operation ordering. If User A completes task-1 and User B starts task-2, the merge won't necessarily reflect the temporal relationship.
- **Divergent IDs** — Two users adding tasks simultaneously may generate the same auto-ID (derived from title words).

**Feasibility:** Git-based sync works for **infrequent, non-overlapping changes** (different users working on different tasks). It breaks down under concurrent modification of the same tasks.

### 6.4 CRDT Approaches

A CRDT (Conflict-free Replicated Data Type) approach would make the graph merge-friendly by design:

- **State-based CRDTs:** Each node is a grow-only set of (field, timestamp, value) triples. Merge = take latest timestamp for each field. Status transitions use a lattice (open < in-progress < done/failed).
- **Operation-based CRDTs:** Instead of replicating state, replicate operations (AddTask, SetStatus, AppendLog). Commutative operations merge without conflict.

**JSONL implications:** The current format stores full node state per line. A CRDT-friendly format might:
1. Add a **vector clock** or **Lamport timestamp** per field
2. Store operations instead of snapshots (operation log → materialized view)
3. Use a **merge function** per field type (max for status lattice, union for log entries, last-writer-wins for title/description)

**Assessment:** CRDT is the correct long-term architecture for multi-machine sync, but it's a significant departure from the current design. For single-machine multi-user (the VPS scenario), it's overkill — flock serialization is sufficient.

### 6.5 Recommended Synchronization Strategy

| Scenario | Strategy | Complexity |
|----------|----------|-----------|
| **Single VPS, shared filesystem** | Current flock model — already works | None |
| **Single VPS, separate WG instances** | Peer IPC (designed, partially implemented) | Low |
| **Multi-machine, low-frequency sync** | Git-based with custom merge driver | Medium |
| **Multi-machine, real-time sync** | Operation log + CRDT merge | High |

The pragmatic path:
1. **Phase 1:** Single VPS with shared `.wg/` — works today
2. **Phase 2:** Add user identity to mutations, multiple coordinators with namespacing
3. **Phase 3:** TCP IPC for cross-machine peer communication
4. **Phase 4:** Operation log format for CRDT-friendly replication (if needed)

---

## 7. Key Architectural Decisions

These decisions gate the multi-user roadmap and should be made before significant implementation:

### Decision 1: Single WG Instance or Per-User WG Instances?

**Option A: Shared single WG instance** — All users operate on one `.wg/graph.jsonl`. Simplest. Already works. Risk: coordinator conflicts if multiple users run coordinators simultaneously.

**Option B: Per-user WG instances with federation** — Each user has their own `.wg/` in their home directory, federated via peers. More isolated but loses the "single graph" collaborative feel.

**Recommendation:** Option A for the initial multi-user experience. It's simpler, already works, and matches the "shared workspace" vision. Add coordinator namespacing to prevent conflicts.

### Decision 2: Web Access Architecture

**Option A: ttyd (terminal-over-web)** — Zero code changes. Each browser session gets its own PTY running `wg tui`. Proven technology.

**Option B: Custom web frontend** — xterm.js component embedded in a web app with additional UI (user presence, notifications, direct links). More polished but significant development effort.

**Option C: TUI screen streaming** — Use the existing `screen_dump.rs` IPC to stream TUI frames to a web viewer. Read-only by default, with input proxying for interaction.

**Recommendation:** Option A (ttyd) for immediate deployment. Option B as a long-term evolution if web-native features are needed.

### Decision 3: User Identity Model

Currently, the graph has no concept of "who" made a change. For multi-user operation, every mutation should be attributed. Options:

**Option A: Environment variable** — `WG_USER` env var, prepended to log entries. Zero code change to the graph format.

**Option B: User field in graph nodes** — Add `modified_by` field to task mutations. Requires graph format extension.

**Option C: Provenance log attribution** — The existing `operations.jsonl` provenance log records all mutations. Add a `user` field there.

**Recommendation:** Option A for immediate use (set `WG_USER` in each user's shell profile), Option C for proper audit trail.

### Decision 4: Real-Time Notification Transport

Current: filesystem watching + polling (50ms-1s latency). Options for lower latency:

**Option A: Status quo** — fs watcher is already fast enough for human perception.

**Option B: IPC broadcast** — Extend the daemon to broadcast "graph changed" events to all connected TUI instances via Unix socket.

**Option C: WebSocket server** — Add a small WebSocket endpoint for push notifications to web clients.

**Recommendation:** Option A is sufficient for the VPS/SSH model. Option C is needed when web access is added (ttyd handles this internally).

### Decision 5: Cross-Machine Replication Strategy

**Option A: Don't — single-machine only** — Defer multi-machine to later.

**Option B: Git sync with merge driver** — Custom merge driver for graph.jsonl conflicts.

**Option C: CRDT graph format** — Redesign storage for conflict-free merging.

**Recommendation:** Option A for now. The VPS model is single-machine by definition. Revisit when the use case demands it.

---

## 8. Risk Assessment

### Easy (works today or minimal effort)

- **Multiple SSH users, each with tmux + TUI** — Already works. flock serialization prevents corruption. fs watcher provides near-real-time updates between instances.
- **Mobile access via Termux/Blink + mosh + tmux** — Already works. Termux-specific code already exists in the TUI.
- **Web access via ttyd** — Zero code changes. Install ttyd, run `ttyd wg tui`, done.
- **Per-user coordinators** — The TUI already supports multiple coordinators with tab switching.

### Medium (requires some development)

- **User identity attribution** — Adding `WG_USER` support to log entries and provenance is straightforward but touches many commands (~20 mutation commands).
- **Coordinator conflict prevention** — Multiple coordinators on the same graph may make conflicting spawn decisions. Needs coordinator-level namespacing or a leader-election mechanism.
- **Responsive TUI for small screens** — The multi-panel layout needs breakpoints for 40-column mobile screens. The rendering code is complex (~7000 lines in `render.rs`) but modular.
- **TOCTOU migration completion** — Ensuring all mutation commands use `modify_graph()` instead of separate load/save. The audit in `file-locking-audit.md` lists the affected commands.

### Hard (significant engineering)

- **Custom web frontend** — Building a web UI around xterm.js with user presence, notifications, and deep-linking. Substantial frontend effort.
- **Cross-machine real-time sync** — Requires either TCP IPC with authentication or a CRDT-based graph format. Neither is trivial.
- **Multi-user access control** — Who can modify which tasks? Currently the graph has no permissions model.
- **Scaling beyond ~10 concurrent writers** — The single-file flock model serializes all writes. Beyond ~10 users, write contention becomes noticeable. Would need the IPC-serialized mutation architecture (Option C from the file-locking audit).

### Might Not Work

- **Real-time collaboration on the same task** — Two users editing the same task description simultaneously. The graph is not designed for character-level collaborative editing. This would require a completely different approach (OT/CRDT at the text level).
- **iOS background sessions** — iOS aggressively kills background apps. Mosh reconnection handles this, but users will see a brief reconnection delay. No workaround within Apple's constraints.
- **NFS/network filesystem** — flock semantics vary across NFS implementations. Some NFS mounts don't support flock at all. The VPS/local-filesystem model avoids this entirely.

---

## 9. Recommended Roadmap

### Phase 1: Multi-User on Shared VPS (works today, polish needed)
1. Document the SSH + tmux + `wg tui` setup as a deployment guide
2. Add `WG_USER` environment variable support for mutation attribution
3. Complete `modify_graph()` migration for all mutation commands
4. Test multiple coordinators on the same graph, add namespacing if needed

### Phase 2: Web Access (ttyd integration)
1. Document ttyd deployment with authentication (reverse proxy + TLS)
2. Test TUI rendering in xterm.js (mouse, color, resize)
3. Add responsive breakpoints for narrow terminals (mobile web)

### Phase 3: Enhanced Multi-User UX
1. User presence indicator in TUI (who's connected, what they're looking at)
2. Task-level notification: "User X completed task Y" as a TUI toast
3. Per-user coordinator isolation with shared visibility

### Phase 4: Cross-Machine (future)
1. TCP IPC with authentication for remote peer communication
2. Git-based sync with custom merge driver for offline/async collaboration
3. Evaluate CRDT graph format if real-time multi-machine sync is needed

---

## References

| Resource | Location |
|----------|----------|
| File locking audit | `docs/research/file-locking-audit.md` |
| Cross-repo communication design | `docs/design/cross-repo-communication.md` |
| Agency federation design | `docs/design/agency-federation.md` |
| Worktree isolation | `docs/WORKTREE-ISOLATION.md` |
| TUI refresh model | `src/tui/viz_viewer/state.rs:3854-4230` |
| File locking implementation | `src/parser.rs:20-283` |
| Federation implementation | `src/federation.rs` |
| Peer commands | `src/commands/peer.rs` |
| Screen dump IPC | `src/tui/viz_viewer/screen_dump.rs` |
| Termux detection | `src/tui/viz_viewer/event.rs:26-50` |
| Mouse mode handling | `src/tui/viz_viewer/event.rs:30-43` |
