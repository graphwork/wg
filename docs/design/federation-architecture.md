# Federation & Cross-workgraph Visibility Architecture

> Each user/team has their own workgraph endpoint. They can see other federated workgraphs. A network of collaborative workspaces.

**Task:** mu-design-federation
**Date:** 2026-03-25

---

## Executive Summary

This document designs the federation model for workgraph: the ability for independent workgraph instances to discover each other, observe each other's state, and optionally dispatch work across boundaries. The design builds on substantial existing infrastructure (peer configs, IPC protocol with `AddTask`/`QueryTask`, agency federation, cross-repo communication design) and maps a path from what works today to a fully federated network.

**Key architectural decision:** Federation is **observation-first, write-optional**. The primary value is visibility -- seeing what's happening across workgraphs. Write capabilities (cross-repo task dispatch) already exist via IPC and are a secondary concern.

---

## 1. What Exists Today

### 1.1 Implemented Infrastructure

| Capability | Status | Location |
|------------|--------|----------|
| Peer config (`federation.yaml` peers section) | **Complete** | `src/federation.rs`, `src/commands/peer.rs` |
| Peer commands (`wg peer add/remove/list/show/status`) | **Complete** | `src/commands/peer.rs` |
| Peer service status detection (PID + socket check) | **Complete** | `src/federation.rs:check_peer_service()` |
| Peer task counting (read remote graph.jsonl) | **Complete** | `src/commands/peer.rs:count_tasks_in_graph()` |
| Agency federation (pull/push/scan/merge/remote) | **Complete** | `src/federation.rs`, 130 tests |
| IPC `AddTask` request (create task on remote graph) | **Complete** | `src/commands/service/ipc.rs:76-97` |
| IPC `QueryTask` request (check remote task status) | **Complete** | `src/commands/service/ipc.rs:99` |
| IPC `GraphChanged` notification | **Complete** | `src/commands/service/ipc.rs:54` |
| Cross-repo dependency syntax (`peer:task-id`) | **Designed** | `docs/design/cross-repo-communication.md` |
| `--repo` flag on `wg add` | **Designed** | `docs/design/cross-repo-communication.md` |
| Task visibility field (internal/public/peer) | **Complete** | `src/graph.rs:303` |
| TUI filesystem watcher (50ms-1s refresh) | **Complete** | `src/tui/viz_viewer/state.rs` |
| TUI coordinator tab system | **Complete** | `src/tui/viz_viewer/render.rs` |
| TUI screen dump IPC (`.wg/service/tui.sock`) | **Complete** | `src/tui/viz_viewer/screen_dump.rs` |

### 1.2 Gap Analysis

| Gap | Why It Matters |
|-----|----------------|
| No cross-workgraph task visibility in TUI | Users can't see peer tasks without CLI commands |
| No periodic peer state refresh | `wg peer status` is point-in-time; no continuous monitoring |
| No visibility filtering on federation reads | Remote graph is read in full -- no filtering by visibility level |
| No authentication on IPC | Unix socket is owner-only (0600), sufficient for local; not for TCP |
| No summary/snapshot format for peer state | Reading full `graph.jsonl` is expensive for large graphs |
| TUI has no concept of "federated view" | Tab bar shows coordinators, not peers |

---

## 2. Discovery Model

### 2.1 How Workgraphs Find Each Other

Three discovery mechanisms, layered from simplest to most automated:

```
  ┌──────────────────────────────────────────────────────────────┐
  │                    Discovery Layers                          │
  │                                                              │
  │  Layer 0: Manual config (wg peer add)          [exists]      │
  │     └── federation.yaml peers: section                       │
  │                                                              │
  │  Layer 1: Filesystem scan (wg peer scan)       [new: v0.1]   │
  │     └── Walk ~/ looking for .wg/ dirs                 │
  │                                                              │
  │  Layer 2: Service announcement                 [future: v0.3]│
  │     └── Running services broadcast presence                  │
  │         via mDNS/DNS-SD or a local registry file             │
  └──────────────────────────────────────────────────────────────┘
```

**Layer 0 (exists):** Manual `wg peer add <name> <path>`. This is sufficient for the common case: a user with 2-5 projects on the same machine, or a small team on a shared VPS. No changes needed.

**Layer 1 (v0.1):** `wg peer scan [<root-dir>]` -- walks a directory tree looking for `.wg/` directories, analogous to `wg agency scan`. Reports found workgraphs with task counts and service status. The user can then `wg peer add` any discovered instances. This mirrors the existing `wg agency scan` pattern.

**Layer 2 (future):** For multi-machine scenarios, services could announce themselves via:
- **Local registry file:** `~/.wg/peers.yaml` as a user-global peer directory
- **mDNS/DNS-SD:** Announce `_workgraph._tcp` on the local network
- **Central registry:** An HTTP endpoint listing known workgraph instances

**Decision:** Start with Layers 0+1. Manual config handles the immediate need; scan reduces discovery friction. Layer 2 is deferred until multi-machine federation is implemented.

### 2.2 Peer Resolution (Existing)

```
peer reference string
       │
       ├── Named peer in federation.yaml?  ──yes──> path from config
       │                                              │
       ├── Absolute path (/ or ~/)?        ──yes──>  canonicalize
       │                                              │
       └── Relative path?                  ──yes──>  resolve from CWD
                                                      │
                                                      ▼
                                              <path>/.wg/
                                                      │
                                              ┌───────┴────────┐
                                              │ state.json?    │
                                              │ PID alive?     │
                                              ├────────────────┤
                                              │ Yes: IPC mode  │
                                              │ No: File mode  │
                                              └────────────────┘
```

This already works in `src/commands/peer.rs:resolve_peer_path()` and `src/federation.rs:check_peer_service()`.

---

## 3. Visibility Model

### 3.1 Existing Task Visibility Levels

Tasks already have a `visibility` field with three levels:

| Level | Meaning | Default |
|-------|---------|---------|
| `internal` | Only visible within the local workgraph | Yes |
| `peer` | Visible to federated peers with credentials | No |
| `public` | Visible to anyone (sanitized view) | No |

**These map directly to federation:**

- **`internal` tasks** are never exposed to peers. This is the default; most tasks stay private.
- **`peer` tasks** are visible to authenticated/configured peers. They see title, status, tags, and a summary description. This is the primary federation visibility level.
- **`public` tasks** are visible to any observer. Sanitized: no internal logs, no agent details, no full descriptions unless explicitly marked.

### 3.2 What Each Visibility Level Exposes

```
┌─────────────────────────────────────────────────────────┐
│                    Visibility Matrix                     │
├──────────────────┬──────────┬──────────┬────────────────┤
│ Field            │ internal │ peer     │ public         │
├──────────────────┼──────────┼──────────┼────────────────┤
│ Task ID          │ local    │ yes      │ yes            │
│ Title            │ local    │ yes      │ yes            │
│ Status           │ local    │ yes      │ yes            │
│ Tags             │ local    │ yes      │ yes            │
│ Dependencies     │ local    │ yes (*)  │ titles only    │
│ Description      │ local    │ summary  │ first line     │
│ Assigned agent   │ local    │ role only│ no             │
│ Logs             │ local    │ no       │ no             │
│ Messages         │ local    │ no       │ no             │
│ Verification     │ local    │ pass/fail│ pass/fail      │
│ Timestamps       │ local    │ yes      │ yes            │
│ Agent output     │ local    │ no       │ no             │
└──────────────────┴──────────┴──────────┴────────────────┘

(*) Cross-repo dependency references are resolved but internal-only
    deps within the remote graph are shown as opaque IDs.
```

### 3.3 Federation View: `PeerTaskSummary`

The unit of data shared across federation boundaries is a **task summary**, not the full task node:

```rust
/// A sanitized view of a task, safe to share across federation boundaries.
/// Derived from a Task node filtered through the visibility model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerTaskSummary {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub tags: Vec<String>,
    /// For peer visibility: description summary. For public: first line only.
    pub description_summary: Option<String>,
    /// Which role is assigned (not the full agent identity)
    pub assigned_role: Option<String>,
    /// Cross-repo dependencies this task has
    pub cross_deps: Vec<String>,
    /// Verification status (if applicable)
    pub verification: Option<VerificationSummary>,
    /// When the task was created
    pub created_at: Option<String>,
    /// When the task last changed status
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationSummary {
    pub required: bool,
    pub passed: Option<bool>,
}
```

### 3.4 Graph Summary: `PeerGraphSnapshot`

For efficiency, peers don't read the full remote graph. Instead, a **snapshot** aggregates the graph state:

```rust
/// Aggregated view of a peer's workgraph state.
/// Generated on demand or cached with a TTL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerGraphSnapshot {
    /// Peer identity
    pub name: String,
    pub path: String,
    /// Service health
    pub service_running: bool,
    pub agent_count: usize,
    /// Aggregate counts
    pub total_tasks: usize,
    pub open: usize,
    pub in_progress: usize,
    pub done: usize,
    pub failed: usize,
    /// Only tasks with visibility >= peer
    pub visible_tasks: Vec<PeerTaskSummary>,
    /// Snapshot timestamp
    pub snapshot_at: String,
}
```

---

## 4. Transport Mechanism

### 4.1 Options Evaluated

| Transport | Latency | Auth | Complexity | Multi-machine |
|-----------|---------|------|------------|---------------|
| **Direct file read** (load peer's graph.jsonl) | ~10ms | Unix perms | None (exists) | No |
| **Unix socket IPC** (existing protocol) | ~1ms | Socket perms | None (exists) | No |
| **TCP IPC** (extend socket to TCP) | ~1-5ms | TLS + tokens | Medium | Yes |
| **HTTP API** (REST or GraphQL) | ~5-20ms | Standard web auth | High | Yes |
| **Git sync** (push/pull graph.jsonl) | ~1-10s | SSH keys | Medium | Yes, async |
| **File sync** (rsync/syncthing) | ~1-30s | Varies | Low | Yes, async |

### 4.2 Selected: Layered Transport

```
┌────────────────────────────────────────────────────────┐
│               Transport Selection                       │
│                                                         │
│  Same machine, same user:                               │
│    Primary:   Unix socket IPC  ← fastest, already works│
│    Fallback:  Direct file read ← no service needed     │
│                                                         │
│  Same machine, different users:                         │
│    Primary:   Unix socket IPC + group permissions       │
│    Fallback:  Direct file read (if readable)            │
│                                                         │
│  Different machines (v0.3+):                            │
│    Primary:   TCP IPC with TLS + token auth             │
│    Fallback:  SSH tunnel to Unix socket                 │
│    Async:     Git sync for offline/eventual consistency │
└────────────────────────────────────────────────────────┘
```

**Rationale:** Unix socket IPC already exists, already has `AddTask` and `QueryTask` handlers, and is the fastest option. For v0.1 the only new work is adding a `QueryGraph` IPC request that returns a `PeerGraphSnapshot`. Everything else is already implemented.

### 4.3 New IPC Request: `QueryGraph`

```rust
/// Request a filtered snapshot of the graph for federation.
QueryGraph {
    /// Only return tasks with this visibility level or higher.
    /// Default: "peer" (skip internal tasks).
    #[serde(default = "default_peer_visibility")]
    min_visibility: String,
    /// Optionally filter by status
    #[serde(default)]
    status_filter: Option<Vec<Status>>,
    /// Optionally filter by tag
    #[serde(default)]
    tag_filter: Option<Vec<String>>,
}
```

Response: `PeerGraphSnapshot` (see section 3.4).

This is the **single new IPC type** needed for federation visibility. `QueryTask` (already exists) provides drill-down into individual tasks.

### 4.4 Data Flow

```
┌──────────────┐     QueryGraph IPC      ┌──────────────┐
│  workgraph A │ ◄──────────────────────► │  workgraph B │
│  (observer)  │                          │  (observed)  │
│              │     PeerGraphSnapshot    │              │
│  TUI/CLI     │ ◄─────────────────────── │  Daemon      │
│  federation  │                          │              │
│  panel       │     QueryTask IPC        │              │
│              │ ◄──────────────────────► │              │
│              │     PeerTaskSummary      │              │
│              │                          │              │
│              │     AddTask IPC          │              │
│              │ ─────────────────────► │              │
│              │     (write, optional)    │              │
└──────────────┘                          └──────────────┘
        │                                         │
        │          Direct file read                │
        └──── (fallback if service down) ─────────┘
              load graph.jsonl, filter by visibility
```

### 4.5 Polling Strategy

Federation state is refreshed on a **configurable poll interval** separate from the graph refresh:

| Context | Interval | Mechanism |
|---------|----------|-----------|
| TUI federated panel visible | 30s (configurable) | Async task polls each peer |
| TUI federated panel hidden | No polling | Conserve resources |
| CLI `wg peer status` | On-demand | Single poll per invocation |
| Coordinator cross-repo dep check | `poll_interval` (60s default) | During coordinator tick |

**No push notifications in v0.1.** Polling is simpler and sufficient for human-scale observation. Push via `GraphChanged` IPC to peers is a v0.2 optimization.

---

## 5. Identity Model

### 5.1 Who Are You Across Workgraphs?

Federation doesn't require a global identity system. Each workgraph instance has a **local name** and an **owner**:

```yaml
# .wg/config.toml (new fields)
[federation]
# Human-readable name for this workgraph instance
name = "workgraph-tool"
# Owner identifier (defaults to $USER)
owner = "erik"
```

When workgraph A queries workgraph B, B's response includes its `name` and `owner` from config. No global ID authority; trust is established through peer configuration (you explicitly added this peer).

### 5.2 For Cross-Repo Task Dispatch

When creating a task on a remote workgraph (`wg add --repo`), the origin is recorded:

```json
{
  "origin": "workgraph-tool:erik",
  "origin_task": "implement-feature-x"
}
```

This provides attribution without requiring a shared user directory.

### 5.3 SSH Keys as Identity (Future)

For multi-machine federation (v0.3+), SSH public keys provide natural identity:
- Already available on developer machines
- Already used for git authentication
- Can be associated with a workgraph owner in `config.toml`
- TCP IPC can use SSH key-based authentication

---

## 6. Read vs. Write Model

### 6.1 Design Decision: Read-First

```
┌─────────────────────────────────────────────────┐
│                Access Model                      │
│                                                  │
│  READ operations (observation):                  │
│    - See task list with status        [v0.1]     │
│    - See aggregate counts             [v0.1]     │
│    - See service health               [exists]   │
│    - Drill into peer-visible tasks    [v0.1]     │
│                                                  │
│  WRITE operations (dispatch):                    │
│    - Create task on remote graph      [exists*]  │
│    - Cross-repo dependencies          [v0.2]     │
│    - Send messages to remote tasks    [v0.2]     │
│                                                  │
│  * AddTask IPC exists, --repo flag designed      │
└─────────────────────────────────────────────────┘
```

**Read is the primary value.** Knowing what's happening across your team's workgraphs is the 80% use case. Write (dispatching tasks to remote graphs) is secondary and already has designed infrastructure.

### 6.2 Why Read-Only Avoids Conflicts

With read-only federation:
- No concurrent write contention across graphs
- No merge conflicts
- No need for distributed consensus
- No need for CRDTs (each graph is authoritative for its own state)
- Simpler security model (observation can't break anything)

Write capabilities exist (`AddTask` IPC) but are opt-in and don't create ownership ambiguity: the receiving graph is authoritative for tasks created within it.

---

## 7. TUI Integration

### 7.1 Federation Panel Design

The TUI gains a new right-panel tab: **Peers** (alongside Chat, Output, CoordLog, etc.).

```
┌─ Graph ───────────────────────────────┬─ Peers ──────────────────────┐
│                                        │                              │
│   ┌─ implement-auth ──────────┐       │  ● workgraph-tool   3/12    │
│   │  Status: in-progress      │       │    ├─ 3 in-progress         │
│   │  Agent: Programmer        │       │    ├─ 5 open                │
│   └───────────────────────────┘       │    └─ 4 done                │
│          │                             │                              │
│          ▼                             │  ● grants-project   1/8     │
│   ┌─ write-tests ────────────┐       │    ├─ 1 in-progress         │
│   │  Status: open             │       │    ├─ 2 open                │
│   │                           │       │    └─ 5 done                │
│   └───────────────────────────┘       │                              │
│          │                             │  ○ data-pipeline    0/15    │
│          ▼                             │    (service stopped)         │
│   ┌─ deploy ─────────────────┐       │                              │
│   │  Status: open             │       │                              │
│   └───────────────────────────┘       │                              │
│                                        │                              │
│                                        │  Last refresh: 12s ago      │
└────────────────────────────────────────┴──────────────────────────────┘
```

### 7.2 Peer Drill-Down View

Pressing Enter on a peer in the Peers panel opens a drill-down showing that peer's visible tasks:

```
┌─ Graph ───────────────────────────────┬─ Peers > workgraph-tool ─────┐
│                                        │                              │
│   (local graph unchanged)              │  ← Back to peers list       │
│                                        │                              │
│                                        │  Tasks (peer-visible):      │
│                                        │                              │
│                                        │  ● implement-federation      │
│                                        │    in-progress [core]        │
│                                        │    Architect agent           │
│                                        │                              │
│                                        │  ● fix-tui-resize            │
│                                        │    open [tui, bug]           │
│                                        │                              │
│                                        │  ✓ add-peer-commands         │
│                                        │    done [federation]         │
│                                        │                              │
│                                        │  3 internal tasks hidden     │
│                                        │                              │
└────────────────────────────────────────┴──────────────────────────────┘
```

### 7.3 TUI Implementation Approach

The federation panel follows the same patterns as the existing coordinator tab system:

1. **State:** Add `PeerViewState` to `VizApp` -- holds cached `PeerGraphSnapshot` per peer, selected peer index, drill-down state.
2. **Tab:** Add `RightPanelTab::Peers` variant. Tab key `P` or position in tab bar.
3. **Async polling:** A background task (similar to `start_fs_watcher()`) polls peers on a 30s interval, updating snapshots. Only polls when the Peers tab is active.
4. **Rendering:** New render function in `render.rs` draws the peers list and drill-down views.
5. **Keybindings:** Enter to drill down, Esc to go back, `r` to force refresh.

### 7.4 Inline Federation Markers (Future)

In v0.2+, cross-repo dependencies could be shown inline in the graph view:

```
  ┌─ use-new-trace ────────────┐
  │  Status: blocked            │
  │  Blocked by:                │
  │    workgraph:impl-trace ◆  │  ◆ = remote dependency
  └─────────────────────────────┘
```

The `◆` marker indicates a cross-repo dependency. Color indicates status (green=done, yellow=in-progress, red=blocked).

---

## 8. Security Threat Model

### 8.1 Threat Surface

```
┌─────────────────────────────────────────────────────────────┐
│                    Threat Model                              │
├────────────────────┬──────────────────┬─────────────────────┤
│ Threat             │ Vector           │ Mitigation          │
├────────────────────┼──────────────────┼─────────────────────┤
│ Unauthorized       │ Read peer's      │ Unix file perms     │
│ graph reading      │ graph.jsonl      │ (owner: 600)        │
│                    │ directly         │ Group: 640 for team │
├────────────────────┼──────────────────┼─────────────────────┤
│ Unauthorized       │ Connect to       │ Socket perms (0600) │
│ IPC access         │ daemon.sock      │ Group socket (0660) │
│                    │                  │ for shared VPS      │
├────────────────────┼──────────────────┼─────────────────────┤
│ Task data          │ PeerGraphSnapshot│ Visibility filter:  │
│ information leak   │ exposes internal │ only peer/public    │
│                    │ task details     │ tasks are returned  │
├────────────────────┼──────────────────┼─────────────────────┤
│ Malicious task     │ AddTask IPC      │ Rate limiting       │
│ injection          │ floods graph     │ (100/s per conn)    │
│                    │ with tasks       │ Origin tracking     │
├────────────────────┼──────────────────┼─────────────────────┤
│ Path traversal     │ Peer path        │ Canonicalize paths  │
│                    │ resolves to      │ Reject symlinks     │
│                    │ sensitive dirs   │ outside scan root   │
├────────────────────┼──────────────────┼─────────────────────┤
│ Replay attacks     │ Cached snapshots │ Timestamp in        │
│                    │ replayed as      │ snapshot; TTL-based │
│                    │ current state    │ cache invalidation  │
├────────────────────┼──────────────────┼─────────────────────┤
│ Man-in-the-middle  │ TCP IPC (future) │ TLS required for    │
│ on network         │ intercepted      │ TCP transport       │
│                    │                  │ (v0.3)              │
├────────────────────┼──────────────────┼─────────────────────┤
│ Denial of service  │ Expensive        │ Snapshot caching    │
│ via federation     │ QueryGraph       │ with min TTL (5s)   │
│ queries            │ requests         │ Connection limiting │
└────────────────────┴──────────────────┴─────────────────────┘
```

### 8.2 Security by Layer

**Same user, same machine (v0.1):** Unix permissions are sufficient. The user owns all `.wg/` directories and sockets. No additional auth needed.

**Shared VPS, multiple users (v0.2):** Create a `workgraph` Unix group. Set socket permissions to `0660` and graph file permissions to `0640` for group members. Visibility filtering ensures `internal` tasks are never exposed even if the file is readable.

**Multi-machine (v0.3+):** TCP IPC requires TLS for transport security and token-based authentication. Tokens are generated per-peer and stored in `federation.yaml`:

```yaml
peers:
  remote-server:
    url: "wg://server.example.com:7432"
    token: "wg_tok_..."
    tls_verify: true
```

### 8.3 Visibility as Security Boundary

The visibility field on tasks is the **primary access control mechanism** for federation. It is enforced at the data layer (in the `QueryGraph` handler), not at the transport layer. Even if an attacker gains socket access, they cannot see `internal` tasks.

**Invariant:** The daemon MUST filter tasks by visibility before serializing the response. This filtering happens in the IPC handler, not in the client.

---

## 9. Minimum Viable Federation (v0.1)

### 9.1 Scope

The smallest useful federation: **see your peers' high-level state from the CLI and TUI.**

| Feature | New Code | Builds On |
|---------|----------|-----------|
| `wg peer scan <dir>` | ~100 lines | `wg agency scan` pattern |
| `QueryGraph` IPC request + handler | ~80 lines | Existing IPC infrastructure |
| Visibility filtering in QueryGraph | ~40 lines | Existing `visibility` field |
| `wg peer tasks <name>` CLI command | ~60 lines | `wg peer show` pattern |
| TUI Peers tab (list view) | ~200 lines | Coordinator tab pattern |
| TUI Peers tab (drill-down) | ~150 lines | Existing panel patterns |
| Federation config in config.toml | ~20 lines | Existing config parsing |
| Snapshot caching (5s TTL) | ~30 lines | Standard cache pattern |

**Total: ~680 lines of new code.**

### 9.2 What v0.1 Delivers

A user with 3 workgraph projects can:
1. `wg peer scan ~/projects` -- discover all workgraphs
2. `wg peer add project-a ~/projects/alpha` -- register peers
3. `wg peer tasks project-a` -- see peer-visible tasks from CLI
4. Open TUI, switch to Peers tab -- see all peers' state at a glance
5. Drill into a peer -- see its visible task list with statuses
6. See the Peers tab auto-refresh every 30s

### 9.3 What v0.1 Does NOT Include

- Cross-repo task dispatch (`--repo` flag) -- designed, defer implementation
- Cross-repo dependencies (`peer:task-id` in `--after`) -- designed, defer
- Push notifications between peers -- polling is sufficient
- TCP transport -- same-machine only
- User identity system -- not needed for single-user
- TUI inline federation markers (the `◆` symbols) -- v0.2

### 9.4 v0.1 Data Flow

```
    User's machine
    ┌────────────────────────────────────────────────────────┐
    │                                                        │
    │  Project A (.wg/)     Project B (.wg/)   │
    │  ┌──────────────┐           ┌──────────────┐          │
    │  │ graph.jsonl   │           │ graph.jsonl   │          │
    │  │ daemon.sock   │◄─────────│ daemon.sock   │          │
    │  │ federation.yaml│ QueryGraph│ federation.yaml│         │
    │  │  peers:       │  IPC      │  peers:       │          │
    │  │   project-b   │──────────►│   project-a   │          │
    │  └──────┬───────┘           └──────┬───────┘          │
    │         │                          │                   │
    │         ▼                          ▼                   │
    │  ┌──────────────┐           ┌──────────────┐          │
    │  │ TUI (user)    │           │ TUI/CLI      │          │
    │  │ Peers tab:    │           │              │          │
    │  │  project-b ●  │           │              │          │
    │  │   3 in-prog   │           │              │          │
    │  │   5 done      │           │              │          │
    │  └──────────────┘           └──────────────┘          │
    │                                                        │
    │  Fallback: if daemon not running,                      │
    │  read graph.jsonl directly (filter by visibility)      │
    └────────────────────────────────────────────────────────┘
```

---

## 10. Implementation Roadmap

### Phase 0.1: Observable Federation (immediate)

1. Add `QueryGraph` IPC request type and handler
2. Implement visibility filtering (only return peer/public tasks)
3. Add `wg peer scan` command
4. Add `wg peer tasks <name>` command
5. Add TUI Peers tab (list + drill-down)
6. Add `[federation]` config section (name, owner)
7. Snapshot caching with TTL

### Phase 0.2: Interactive Federation

1. Implement `--repo` flag on `wg add` (dispatch via `AddTask` IPC)
2. Cross-repo dependencies (`peer:task-id` in `--after`)
3. Coordinator resolves cross-repo deps during tick
4. TUI inline markers for cross-repo dependencies
5. Push notifications (`GraphChanged` to peers on task completion)
6. `wg peer tasks --watch` for continuous monitoring

### Phase 0.3: Multi-Machine Federation

1. TCP IPC listener (alongside Unix socket)
2. TLS encryption for TCP transport
3. Token-based authentication
4. `wg peer add` accepts URLs (`wg://host:port`)
5. SSH tunnel helper (`wg peer tunnel <name>`)
6. User-global peer registry (`~/.wg/peers.yaml`)

### Phase 0.4: Advanced Federation

1. Git-based async sync for offline collaboration
2. Custom merge driver for graph.jsonl conflicts
3. Federated agency sync (auto-pull on coordinator tick)
4. Cross-repo trace function sharing
5. Federation dashboard (aggregate view across all peers)
6. mDNS/DNS-SD service announcement

---

## 11. Mapping to Existing Visibility Levels

The existing `internal`/`public`/`peer` visibility levels were designed with federation in mind. Here's how they integrate:

### 11.1 Task Creation

```bash
# Default: internal (not visible to peers)
wg add "Fix internal bug"

# Visible to configured peers
wg add "Implement shared API" --visibility peer

# Visible to anyone (including unknown observers)
wg add "Project milestone" --visibility public
```

### 11.2 Federation Query Flow

```
Peer A sends QueryGraph(min_visibility="peer") to Peer B
    │
    ▼
Peer B's daemon loads graph.jsonl
    │
    ▼
Filter: task.visibility ∈ {"peer", "public"}
    │
    ▼
For each matching task, create PeerTaskSummary:
    ├── peer visibility: include description summary, assigned role
    └── public visibility: include first line of description only
    │
    ▼
Return PeerGraphSnapshot with filtered tasks + aggregate counts
```

### 11.3 Visibility Promotion

Tasks can be promoted to higher visibility:

```bash
# Make an existing task visible to peers
wg edit <task-id> --visibility peer

# Make it publicly visible
wg edit <task-id> --visibility public

# Demote back to internal
wg edit <task-id> --visibility internal
```

The coordinator could automatically promote tasks based on rules (e.g., all tasks with tag `shared` get `peer` visibility).

---

## 12. Design Decisions Summary

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| 1 | Discovery | Manual config + filesystem scan | Simplest; handles 90% of cases |
| 2 | Transport (v0.1) | Unix socket IPC | Already exists; fastest; no auth needed |
| 3 | Transport (v0.3+) | TCP IPC with TLS | Natural extension of existing IPC |
| 4 | Data format | `PeerGraphSnapshot` / `PeerTaskSummary` | Filtered, efficient, privacy-preserving |
| 5 | Visibility mapping | Reuse existing internal/peer/public | Already designed into the graph model |
| 6 | Read vs write | Read-first; writes via existing `AddTask` | Avoids distributed consistency problems |
| 7 | TUI integration | Peers tab with drill-down | Consistent with coordinator tab pattern |
| 8 | Polling strategy | 30s interval, active-tab-only | Balances freshness with resource use |
| 9 | Identity | Local name+owner from config | No global identity authority needed |
| 10 | Security | Visibility filtering + Unix perms | Defense in depth; simple for v0.1 |

---

## References

| Resource | Location |
|----------|----------|
| Research: multi-user TUI feasibility | `docs/research/multi-user-tui-feasibility.md` |
| Design: agency federation | `docs/design/agency-federation.md` |
| Design: cross-repo communication | `docs/design/cross-repo-communication.md` |
| Federation implementation | `src/federation.rs` |
| Peer commands | `src/commands/peer.rs` |
| IPC protocol | `src/commands/service/ipc.rs` |
| Task visibility field | `src/graph.rs:303` |
| TUI coordinator tabs | `src/tui/viz_viewer/render.rs:1896` |
| File locking audit | `docs/research/file-locking-audit.md` |
