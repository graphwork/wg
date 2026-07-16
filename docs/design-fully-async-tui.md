# Fully asynchronous TUI on slow storage and large graphs

Status: proposed design for `implement-nonblocking-tui` and its follow-on
stages. This document describes the current code as of July 2026.

## Decision

WG should put an in-process `SnapshotEngine` between the TUI and every source
of project state. The engine is available even when the coordinator daemon is
stopped. It performs storage reads, parsing, enrichment, indexing, layout, and
width-dependent text projection away from the input/render thread, then
publishes immutable, versioned snapshots. A later daemon snapshot protocol may
provide the same snapshots as an optimization; it is not the correctness path
and is never awaited before painting.

The design has one enforceable invariant:

> Once the `tui` command is selected, the input/render thread performs no
> filesystem access, blocking IPC, subprocess work, or computation whose cost
> grows without a viewport-sized bound. It may mutate small interaction state,
> make non-blocking bounded-channel calls, swap an `Arc` to an already-derived
> snapshot, and render at most the visible rows and columns.

This is stronger than saying that graph reads are asynchronous. It covers
metadata calls, configuration and registry loads, recursive directory walks,
history and log tails, parsing, token/message accounting, filtering, sorting,
DAG analysis, layout, wrapping/markdown generation, and application of a large
result. A cache lookup does not satisfy the invariant if validating that cache
performs `metadata(2)` or if applying the hit is proportional to graph size.

## Performance contract

The implementation is accepted against all of these targets, measured in a
120x40 PTY on an otherwise idle CI runner:

| Surface | Required target |
| --- | --- |
| First frame | A neutral, useful skeleton is painted within 50 ms p99 after terminal setup, and within 100 ms p99 from dispatch of `Commands::Tui`. This remains true when every project-storage operation is delayed by 1, 3, or 5 seconds. |
| Input | A key changes focus, input text, or the on-screen key acknowledgement within 50 ms p99 and never more than 100 ms, including during a slow read, continuous graph mutation, and snapshot publication. |
| Frame | Render p95 <= 16.7 ms and p99 <= 33 ms. Input dispatch uses <= 2 ms; accepting async results uses <= 1 ms per frame. |
| Scale | 10,000 tasks, 50,000 dependency edges, 100 active agents, a 25 MiB `graph.jsonl`, a 100 MiB log, and 100,000 chat-history records do not change the first-frame or input targets. |
| Refresh | On local SSD, a full 10k/50k base-graph read, parse, index, and default layout completes within 2 seconds p95 in release mode. Publication is coalesced to at most 5 Hz. Slow network storage may take longer without consuming the UI budgets. |
| Memory | At most two graph generations plus bounded projections are retained. Default TUI-owned snapshot/cache memory is 256 MiB; log/history pages are at most 200 records or 1 MiB, and a file preview reads at most 1 MiB/1,000 lines. |
| Shutdown | Terminal restoration starts immediately and completes within 100 ms when a worker is stuck in a 5-second storage call. Process exit is <= 250 ms; correctness must not depend on joining a stuck reader or rewriting all histories at exit. |

The first frame is deliberately independent of whether `.wg` exists. It can
show the project path, tab chrome, empty panels, and `Loading` from values
already supplied by CLI parsing. Configuration does not need to be known to
paint it.

## Current behavior and measured baseline

### Method

The baseline used sparse synthetic graphs with 1,000, 5,000, and 10,000 tasks
(chains of 50, so the layout fixture was nontrivial without being adversarial),
plus this repository's real 306-task graph. Measurements used the installed
release `wg`, `tmux` at 120x40, and `wg tui-dump`. Time to first usable frame was
measured from process launch until `tui-dump` returned a nonempty screen.
Keystroke latency was measured from `tmux send-keys` until the dump contained
the requested command-mode help panel.

A temporary `LD_PRELOAD` test shim delayed pathname-based `open*`, `statx`, and
`fstatat` operations. The decisive run delayed only pathnames containing
`graph.jsonl` by one second; all other project and system paths stayed local.
This is a conservative lower bound, not an emulation of all NFS behavior.
`strace -f -T` supplied syscall counts. The temporary generator, shim, and
source probes were removed after measurement.

| Fixture / operation | Observed result |
| --- | --- |
| Synthetic first usable frame, local filesystem | 1k: 220 ms median; 5k: 546 ms; 10k: 927 ms (three runs each) |
| Synthetic 10k help-key response, local filesystem | 249 ms |
| `wg viz` on synthetic graph | 1k: 0.02 s; 5k: 0.07 s; 10k: 0.15 s |
| `wg viz` on repository graph | 0.75 s warm wall median, about 0.69-0.72 s user CPU and 0.04-0.05 s system time |
| Repository `wg viz` syscall trace | 7,374 traced syscall lines; 3,075 referenced the project `.wg` path |
| Synthetic 10k `wg viz` syscall trace | 37,547 traced lines and 37,510 project-path calls, almost all metadata calls |
| 1k TUI startup, only `graph.jsonl` path operations delayed by 1 s | first socket at 11.19 s; first usable frame at 11.23 s |
| Post-start graph invalidation under the same delay, then help key | 5.01 s until the help panel appeared |

The local repository result is CPU dominated, while the synthetic syscall
trace shows that enrichment scales metadata traffic even when task logs and
messages are absent. The injected result attributes at least eleven serial
graph-path operations to startup and at least five to one refresh/interaction
episode. A real network filesystem also delays configuration, directories,
agents, messages, history, and logs, so these are lower bounds.

The existing async tests do not exercise this failure. The unit tests
`main_thread_api_never_blocks` and the ignored
`main_thread_api_unblocked_under_simulated_500ms_latency` in
`src/tui/viz_viewer/async_fs.rs` prove only that calls into `AsyncFs` enqueue
quickly. The smoke scenario
`tests/smoke/scenarios/tui_responsive_under_500ms_latency.sh` invokes those
APIs without launching the TUI. The cache benchmarks in
`tests/integration_tui_perf_benchmarks.rs` cover message-stat and token-usage
cache hits, but cache validation can still issue metadata calls and the real
event/render paths bypass `AsyncFs` in many places.

## Blocking inventory

“UI” below means the thread running `run_event_loop_inner`. “Worker” means work
already sent to the current `AsyncFs` or an agent-tail thread. Line numbers are
guideposts; symbols are the stable references.

### Bootstrap before the first frame

| Path | Current thread and work | Risk |
| --- | --- | --- |
| `Commands::Tui` in `src/main.rs` around line 3353 | UI/bootstrap: `Config::load_or_default` before `viz_viewer::run` | Global/local config can block before terminal paint. |
| `viz_viewer::run` in `src/tui/viz_viewer/mod.rs:59` | UI/bootstrap: keyboard capability query, optional `EventTracer::new`, then `VizApp::new` | Trace creation opens a file synchronously; terminal negotiation also needs a bounded timeout. |
| `VizApp::new` in `state.rs` around line 6907 | UI/bootstrap: graph metadata, config load, large state construction | The async worker is constructed only after synchronous setup has begun. |
| `VizApp::new` startup chain | UI/bootstrap: recursive watcher registration, graph load, `generate_viz_output_from_graph`, stats, saved TUI state, user-coordinator discovery, tab sync, agent monitor/dashboard, coordinator/service/vitals/time checks, every chat history, and optional PTY discovery | Repeated graph/config/registry loads plus parse, accounting, layout, history reads, process checks, and mutation are all ahead of the first frame. |
| `screen_dump::start_server` in `mod.rs:152` / `screen_dump.rs:217` | UI/bootstrap: creates `.wg/service`, checks/removes `tui.sock`, and binds before the event loop | A diagnostic facility can delay the first screen on the same slow storage. |
| `run_event_loop_inner` in `event.rs:350` | UI: calls `maybe_refresh` before the first `terminal.draw` | Even a minimal constructor would still allow refresh work to precede the first frame. |

### Refresh, input, and render

| Surface | UI-thread work today | Existing off-thread portion / remaining problem |
| --- | --- | --- |
| Refresh | `maybe_refresh` (`state.rs:8889`) drains `AsyncFs`, then `apply_loaded_graph_refresh` regenerates visualization/stats and reloads monitor, firehose, detail, logs, and panels. Watch/timer branches call chat polling, service health, vitals, status, config, and panel loaders directly. | Only graph/stat/stream reads submitted through `AsyncFs` are off-thread. Parse, derive, layout, application, and most reads remain on UI. |
| Result application | `apply_viz_result` (`state.rs:7207`) clones/formats lines, builds plain/search lines and ID/line maps, annotations, widths, animation state, ordering, selection, and trace. `load_stats_from_graph` (`state.rs:8417`) scans tasks, registries, live token logs, archive lines, cycles, and message status. | A worker-delivered `WorkGraph` can therefore cause a long UI stall after I/O finishes. |
| Input queue | `run_event_loop_inner` drains `rx.try_recv()` until empty after the first event. | The reader is off-thread and the channel is bounded, but an unbounded per-frame drain lets paste/event storms starve render. |
| Search/filter/sort/trace | `update_search`/`rerun_search` fuzzy-score graph lines and sort results; `apply_sort_mode` sorts task order; `recompute_trace` traverses dependencies. | All execute in key handlers. Complexity grows with graph/result size. |
| Render entry | `render::draw` directly calls `load_hud_detail`, `load_log_pane`, `load_messages_panel`, lifecycle/coord/activity loaders, `FileBrowser::new`, and `update_firehose` (`render.rs:122-157`). Right-tab rendering repeats loaders around lines 2347-2359. | Rendering is impure and can block on a single draw. |
| Runtime/config render | `build_coordinator_runtime_lines` (`render.rs:5273`) loads config, compactor state, inbox/outbox, and summary existence. Config rendering loads config again around line 9894. | These are repeated filesystem calls in frame generation. |
| Graph render | Visible graph rows are sliced, but coordinator/user-board line sets and task classification scan large maps repeatedly; some classification is effectively viewport x graph. | Render needs a prebuilt row classification/projection and O(viewport) lookup. |
| Chat render | `draw_chat_tab` (`render.rs:3075`) rebuilds markdown/wrapping for all loaded messages before viewport slicing. Response annotation scans later messages for each user message, yielding quadratic history behavior in the worst case. | Width-keyed, paged render projections must be worker-built. |
| Other text render | Detail, logs, activity, and stream views wrap or parse all cached lines per frame. | “Cached bytes” are not a render-ready bounded snapshot. |
| Trace and clipboard | `EventTracer::record` (`trace.rs:78`) writes from input handling. Clipboard helpers can run external programs and temporary-file/attachment I/O from key handlers. | Both belong behind async commands; clipboard completion is a result, not an inline action. |
| Exit | `run_event_loop` calls `save_all_chat_state` (`event.rs:342`, `state.rs:14181`) before terminal restoration. | It reloads config and may rewrite histories for every coordinator, so a hung filesystem traps the terminal in raw mode. |

### Graph CPU and enrichment

`generate_viz_output_from_graph` in `src/commands/viz/mod.rs:385` is not a pure
layout over an in-memory graph. It analyzes cycles and components, walks filter
ancestors, resolves cross-peer status, scans live/archived task and agency logs
for token usage, and queries message statistics. The “cached” token and message
paths still validate files with metadata calls.

`ascii::generate_ascii` performs another cycle/adjacency/layout pass. In
`src/commands/viz/ascii.rs:148-169`, every fan-in node constructs an ancestor
set for each parent and repeatedly intersects those sets. On wide, dense DAGs
this is superlinear and can approach quadratic work. Component ordering,
channel routing, and the character edge map also grow with graph and rendered
edge size. None of it may execute or be applied incrementally on the UI thread.

The new worker must separate phases explicitly:

1. read stable bytes;
2. parse graph/config/registry inputs;
3. build graph indexes, cycle/component analysis, message/token/accounting maps;
4. apply filter/sort/search/trace predicates;
5. compute DAG/layout and row model;
6. build width-dependent visible text projections;
7. publish an immutable snapshot.

This separation is required both for telemetry and cooperative cancellation.

### Detail, log, messages, monitor, archive, and browser paths

| Surface and symbol | Blocking / unbounded work on UI today |
| --- | --- |
| HUD detail: `load_hud_detail` / `load_hud_detail_for_task` (`state.rs:9716`, `10554`) | Finds archives recursively, reloads graph and registry, reads session summaries, journals, agency YAML, prompts, outputs, evaluation directories/files and token logs, parses/derives markdown, and collects metadata. |
| Task log: `load_log_pane` (`state.rs:11133`) | Reloads graph, enumerates attempts/history, opens and parses log data. `update_log_output`, stream-event and output-pane updates perform direct metadata/open/seek/read/parse. New suffixes are not intrinsically bounded. |
| Coordinator/activity/firehose | `load_coord_log`, `load_activity_feed`, and `update_firehose` (`state.rs:13301`) perform incremental I/O on UI. Incremental offsets limit repeated bytes but do not cap a newly appended burst. |
| Messages: `load_messages_panel` (`state.rs:11591`) | Reloads graph, reads/parses the message JSONL, builds/sorts all entries, then writes the read cursor synchronously. |
| Agent monitor/dashboard: `load_agent_monitor` / `load_dashboard` (`state.rs:12688`, `12742`) | Reload graph and registry/coordinator state, stat every agent output, inspect child processes, sort/build all rows, and calculate status/vitals. Agent stream-tail threads are a useful partial precedent, but dashboard and accounting remain synchronous. |
| Chat startup/poll: `load_chat_history`, `poll_chat_messages` (`state.rs:14045`, `14390`) | Loads config/graph to discover chats, reads histories/outboxes/archives, reads streaming state, and rewrites history after polling. Startup preloads all coordinators. |
| Chat pagination | `load_jsonl_tail` (`state.rs:3195`) seeks to the end but then reads the entire file; `load_jsonl_page` (`state.rs:3276`) also reads and splits the whole file for each page. `load_archive_history` (`state.rs:14286`) reads every archive and sorts all messages. |
| Chat persistence | `save_chat_history_with_skip` loads config, may read all prior history, serializes, and rewrites the file. It is called during polling and `save_all_chat_state`. |
| Archive browser: `ArchiveBrowserState::load` (`state.rs:4038`) | Reads/parses all archive JSONL records. Each filter keystroke lowercases and filters the entire collection. |
| File browser: `FileBrowser::refresh` (`file_browser.rs:107`) | Recursively walks `.wg`, calls `is_dir`, and sorts each directory. Search repeats the walk on each query change. |
| File preview: `FileBrowser::load_preview` (`file_browser.rs:179`, also called from `file_browser_render.rs:46`) | Stats and reads the whole file despite the display cap, counts all lines, lazily initializes syntax definitions/themes, and highlights up to 1,000 lines. Render may trigger it. |
| Forms/settings/history browser | Constructors and open/save handlers reload graph, config, profiles, session/history segments, and sometimes write config/state on the UI thread. A task form reloads the graph just to populate dependency choices. |

## Why the current `AsyncFs` boundary is insufficient

`AsyncFs` (`src/tui/viz_viewer/async_fs.rs:97`) is one thread fed by two
unbounded `std::sync::mpsc` channels. `RequestKey` deduplicates only graph,
stat, and streaming requests; chat-interaction mutations are not keyed.
Consequences:

- a multi-second graph open causes head-of-line blocking for chat, status, and
  interaction updates;
- an event storm can grow the request and response queues without bound;
- there is no request generation, cancellation token, or revision vector, so
  an older completion can be applied after a newer user selection or mutation;
- the shared cache is mutable under mutexes, while the UI still performs the
  expensive derivation and snapshot application;
- the slow-operation indicator sees only work routed through this one worker,
  leading the status bar to claim the TUI is nonblocking while direct loaders
  are stalled;
- `Drop` sends `Shutdown`, but there is no shutdown contract for a worker stuck
  in an uninterruptible network filesystem syscall.

The useful parts to retain are nonblocking submission, request-key
deduplication, and the agent-tail precedent. They should be generalized into a
bounded snapshot pipeline rather than extended one path at a time.

## Snapshot architecture

### Components

```text
terminal events ──> UI model ──try_send──> Request broker
     ^                 │                       │
     │                 │ Arc swap              ├─ interactive I/O lane
     │                 │                       ├─ bulk I/O lane
     │             bounded results <───────────┤
     │                                         └─ bounded CPU pool
     └── render viewport from Arc<UiSnapshot>

optional coordinator daemon ──versioned snapshot IPC──> Request broker
project filesystem ───────────StorageBackend───────────> I/O lanes
```

The UI starts with `UiSnapshot::empty(project_label)` and paints it before
creating a watcher, dump socket, daemon client, or project storage worker.
`Commands::Tui` must stop loading `Config`; color and key settings arrive as a
later config snapshot. `VizApp::new` becomes a pure, bounded constructor.
After the first successful draw, it nonblockingly starts:

- a request broker with bounded queues;
- an **interactive I/O lane** for the selected detail, active chat page,
  cursor mutation, and active log tail;
- a **bulk I/O lane** for graph/config/registry discovery, archive scans, file
  search, inactive histories, and large reconciliation reads;
- a bounded CPU pool for parse, index, accounting, search/sort, layout, syntax
  highlighting, markdown/wrapping, and projection;
- either a bounded watcher plan or polling scheduler;
- the dump server and optional daemon IPC probe.

Separate I/O lanes matter because a thread inside a blocking NFS syscall
cannot be cooperatively cancelled. A slow bulk graph read must not prevent a
chat keystroke from being accepted or its durable mutation from being queued.
The initial implementation may use two dedicated threads and a two-thread CPU
pool. It must not create an unbounded thread per request.

All source access is through a `StorageBackend` trait so latency, reordering,
stale attributes, short reads, rename races, and errors can be injected in
tests. Production's filesystem implementation may remain blocking because it
runs only on the I/O lanes.

### Immutable data products

The engine publishes these products independently so a slow enrichment does
not hold back the base graph:

```rust
struct SnapshotStamp {
    snapshot_id: u64,       // monotonically assigned by this engine instance
    generation: u64,        // desired generation for the request key
    project_id: ProjectId,
    revisions: RevisionVector,
    schema_version: u32,
}

struct RevisionVector {
    graph: Option<ContentRevision>,
    config: Option<ContentRevision>,
    agents: Option<ContentRevision>,
    messages: Option<ContentRevision>,
    chat: Option<ContentRevision>,
    logs: Option<ContentRevision>,
}

struct UiSnapshot {
    stamp: SnapshotStamp,
    base: Arc<BaseGraphSnapshot>,
    enrichment: Arc<EnrichmentSnapshot>,
    graph_view: Arc<GraphProjection>,
    panels: Arc<PanelSnapshots>,
}
```

`ContentRevision` contains a source sequence when available plus length,
file-identity information, and a content digest. It must not order NFS updates
by mtime alone. `BaseGraphSnapshot` owns parsed nodes and immutable indexes by
task ID, status, assignee, parent/child, component, and cycle. Enrichment owns
token/message/accounting maps. `GraphProjection` is keyed by filter, sort,
search, trace, layout mode, terminal width, and source revisions. Panel
snapshots are keyed by panel kind, selected stable ID, page/tail cursor, width,
and revisions.

Snapshots use `Arc`; publication is an O(1) pointer swap. The UI never copies
ten thousand rows when accepting a result. Selection, scroll anchors, open
dialogs, editor buffers, unsent chat drafts, and optimistic mutation overlays
remain UI-owned. Reconciliation looks up their stable IDs in prebuilt indexes;
it does not scan the graph. If an ID disappeared, a bounded policy selects its
nearest precomputed neighbor.

The base graph can become visible while token/message enrichment says
`pending`. A new enrichment revision may update badges without replacing the
layout. The UI is explicitly eventually consistent: it displays the latest
coherent product for each region, labels a retained older product as
refreshing/stale when appropriate, and converges after notifications or polling
quiesce. It never combines indexes with graph bytes from a different revision.

### Requests, coalescing, and backpressure

Every request has a semantic key, desired generation, priority, input revision,
and cooperative cancellation token:

```rust
Request {
    key: RequestKey,             // GraphBase, GraphView(ViewKey), Panel(...), ...
    generation: u64,
    priority: Priority,
    input_revisions: RevisionVector,
    cancel: CancellationToken,
}
```

Default queue limits are 32 control/mutation requests, 32 interactive reads, 8
bulk reads, and 32 completed results. UI and watcher code use `try_send`; they
never wait for capacity.

For each key the broker retains at most one running request and one latest
pending request. Enqueuing generation `g+1` replaces any pending `g` and marks a
running `g` cancelled. Refresh hints and progress messages may be dropped or
coalesced. Durable mutations are never dropped: their bounded control queue
uses an on-disk command/journal writer on the interactive lane; if saturated,
the UI retains a visible pending overlay and retries rather than blocking or
claiming success.

Workers check cancellation between read chunks, JSONL parse batches, index
phases, search/sort batches, component layout, and text-page projection. A
blocking syscall itself may finish late. Its result is harmless because the UI
accepts a completion only when all of these hold:

1. `result.generation == desired_generation[result.key]`;
2. its project and schema match;
3. its view/panel key still matches current UI intent;
4. its input revision is still the revision selected for that product; and
5. it does not cross a mutation fence.

The event loop drains at most eight completions or one millisecond of result
work per frame, whichever comes first. It similarly dispatches at most 64
terminal events or two milliseconds before drawing, preventing paste storms
from starving render.

### Mutations and read races

Mutation commands use the existing background command mechanism where
possible, but all direct config, cursor, interaction, and chat-history writes
move behind the control lane. Submission increments a per-domain mutation
fence before the write starts. Results based on an older fence are rejected.
The UI keeps an optimistic overlay identified by command ID:

- success schedules a read-after-write reconciliation and removes the overlay
  only when a snapshot observes the command's revision;
- failure marks the overlay failed and retains the user's editor/draft for
  retry;
- timeout is “unknown, reconciling,” not success or failure;
- a newer external graph does not erase a local unsent edit.

For `graph.jsonl` atomic replacement, a worker opens one file descriptor,
reads to a bounded buffer/stream parser, records `fstat` identity and length,
and computes a digest. It then checks the current path revision. If the path
was replaced, truncated, or a watcher/poll already observed a newer digest,
the result is rejected and the latest generation is queued. Delete/recreate,
mtime rollback, and equal-mtime replacements are therefore safe. There is no
assumption that notify delivery is ordered or complete.

## Watching and polling network filesystems

`start_fs_watcher` currently registers `RecursiveMode::Recursive` on the whole
workgraph directory (`state.rs:8775-8812`). Registration itself is synchronous
and may traverse a large remote tree. It also watches high-churn agent outputs,
archives, histories, caches, and service files WG does not need for the active
screen. This registration is eliminated.

Stage 1 should use coarse polling only; it is predictable on NFS, MooseFS, and
sshfs and is sufficient for correctness. Polling runs on the I/O lanes and is
coalesced by request key:

- active chat/log/detail paths: nominal 1 second;
- `graph.jsonl`, effective config inputs, agents registry, messages directory,
  archive index, and service state: 2 seconds;
- inactive tabs and histories: 5 seconds;
- immediate refresh hint after a successful WG mutation.

Intervals back off to 10 seconds after repeated slow/error results and recover
gradually. Only one probe per domain is in flight. Content revision, not mtime,
decides equality. Polling never delays input or frame cadence.

A later local-filesystem optimization may install nonrecursive watches, on a
worker and only after first paint, for this explicit plan:

- `.wg` nonrecursive, filtering direct durable files such as `graph.jsonl`,
  `config.toml`, and archive index;
- `.wg/messages`, `.wg/service`, and `.wg/log`, each nonrecursive;
- the active chat session/history directory;
- `.wg/agents` nonrecursive plus one nonrecursive watch for each active agent
  directory, capped at 64 by default and 128 hard maximum.

If the cap is exceeded, registration takes longer than 100 ms, notify reports
overflow, the filesystem type is known remote, or any watch fails, that domain
uses polling. Even a healthy local watch gets a 30-second reconciliation poll
because notify events can be lost. No production code may request
`RecursiveMode::Recursive` beneath `.wg`; a watcher-plan unit test and source
guard enforce that constraint.

## Loading and slow-storage feedback

Feedback occupies one compact status slot. It is not a stream of warnings or
toasts. The state machine is:

```text
Cold --first draw--> Loading --snapshot--> Ready
                         |                    |
                  threshold/error       refresh hint
                         v                    v
                       Slow <---------- Refreshing
                         |                    |
                         +----result--------> Ready
                         +----error---------> StaleRetrying
                                                |
                                             recovery
                                                v
                                              Ready
```

`Loading` is initially silent so fast storage does not flash. After 150 ms the
slot shows a phase such as `Loading · discover`, `Loading · graph`, `Building
view · indexes`, `Building view · layout`, or `Loading · chat`. After 750 ms in
an I/O phase it makes one episode transition to `Storage slow · graph (3.2s)`.
A slow CPU phase says `Building view · layout (3.2s)`, not “storage slow.”
Phase progress is coalesced to at most four updates per second.

On refresh, the last good snapshot stays interactive. `Refreshing · graph`
appears only after 250 ms, becomes slow after one second, and clears on
recovery. An error shows `Stale · retrying (graph)` while retaining the last
good data; initial-load errors show the empty state plus a retry action. One
episode produces at most one normal-to-slow transition and no repeated toast.
Changing phases does not reset the slow timer or generate another warning.
Successful recovery clears the slot (or returns it to `Ready`) and resets the
episode dedup key.

Progress events themselves use a one-slot latest-value channel, so feedback
cannot exert backpressure on work or rendering.

## Data ownership and future local-state migration

The asynchronous fix does not require relocating files. `StorageBackend` first
reads today's paths and caches remain disposable. It does, however, establish a
resolver that permits a later split:

| Durable shared project state (remain on project filesystem) | Volatile/per-user state (eligible for local state/cache) |
| --- | --- |
| `graph.jsonl`, project config, task messages and their durable delivery state, authoritative chat inbox/outbox/history, archives, task artifacts and logs needed for audit/recovery | TUI socket, screen dump state, trace buffers, panel/scroll preferences, parsed graph/index snapshots, token/message indexes, layouts, wrapped markdown/syntax projections, tail offsets, rebuildable read accelerators, transient streaming mirrors, service PID/runtime markers |

Local persistent state belongs under
`$XDG_STATE_HOME/wg/<project-id>/`; rebuildable data belongs under
`$XDG_CACHE_HOME/wg/<project-id>/`. `project-id` is derived from a stable
project identity, not merely an NFS mount spelling. Moving an item requires a
per-item compatibility decision: dual-read local-then-legacy, write the new
location, and tolerate invalid/missing caches. Durable shared truth is never
inferred from a local cache.

Until migration, screen-dump socket creation and TUI state writes still go
through the async lanes after first paint. Moving the socket and trace locally
is an early low-risk follow-up, but is not a prerequisite for satisfying the
thread invariant.

## Daemon IPC is an accelerator, not the boundary

The coordinator already observes much of the graph, so a later protocol can
avoid duplicate parsing. On startup the broker makes a nonblocking IPC probe.
If the daemon is absent, stopped, hung, disconnected, or incompatible, the
in-process engine continues unchanged. There is no modal warning for an absent
daemon.

Daemon snapshots use the same immutable product schema and include protocol
version, project identity, source revision vector, generation, and content
digests. The client rejects a mismatched schema/project, a revision older than
its mutation fence, or a stale generation. Disconnect falls back to filesystem
polling while retaining the last good snapshot. Reconnect can replace products
only through the same acceptance rules. The daemon must not push unbounded
event streams into the UI.

This staged combination is preferable to daemon-only IPC because `wg tui` is a
diagnostic and management surface when the daemon is broken—the exact time it
must still work. It is preferable to sharing mutable graph objects because the
immutable wire/product boundary also solves CPU placement, stale results, and
large application costs.

## Failure and compatibility behavior

- Graph/config/message formats and existing CLI mutations remain unchanged.
- Missing or corrupt input publishes a typed error for that domain. The last
  coherent snapshot remains visible; first boot remains an interactive empty
  shell. Retries back off and recover automatically.
- A full queue coalesces refresh work. It never blocks the UI and never silently
  reports a mutation as committed.
- A worker panic marks its lane failed and restarts it after backoff. The UI
  retains snapshots and reports one stale episode.
- A stuck filesystem call consumes one fixed lane, not the UI or an unbounded
  thread. Interactive and bulk isolation preserves high-priority work.
- Watcher failure and NFS notification loss are normal polling cases.
- The UI continuously debounces persistence of drafts/preferences rather than
  doing a mandatory full flush at exit. Durable chat/message writes retain
  command acknowledgements. On shutdown it cancels work, closes result
  receivers, starts terminal restoration, and does not join stuck lanes.
- Width/theme changes request a new projection and keep the old projection (or
  a bounded plain placeholder) until ready. They never rewrap the full history
  inline.
- Large result arrival cannot reset focus, scroll, dialog, or draft state;
  stable-ID reconciliation and mutation fences are part of acceptance.

## Rollout

### Stage 0: keep measurement honest

Add phase spans for `bootstrap.shell`, `watch.register`, `read.graph`,
`parse.graph`, `derive.indexes`, `enrich.tokens`, `enrich.messages`,
`layout.ascii`, `project.text`, `publish.snapshot`, `ui.accept`,
`input.dispatch`, and `render.frame`. Tracing writes through a bounded local
collector, never the project filesystem on the UI thread. Every performance
test emits p50/p95/p99 and maximum, not only a total wall time.

### Stage 1: nonblocking shell and base graph

- Make the `Commands::Tui` arm and `VizApp` constructor storage-free.
- Draw before `maybe_refresh`, dump-server startup, watcher/poller startup, or
  daemon probing.
- Introduce `StorageBackend`, bounded broker, two I/O lanes, generation IDs,
  mutation fences, feedback state, and immutable base/config snapshots.
- Route startup graph/config/state/agent/chat discovery through the engine.
- Debounce exit state throughout the session and restore the terminal without
  waiting.

This is the minimum implementation task. It delivers the first-frame and
keystroke guarantees even though some tabs may initially show async
placeholders.

### Stage 2: graph derivation and publication

- Make graph parsing, cycles/components, indexes, accounting, filter/sort/search,
  trace, DAG/layout, row classification, and text projection worker-only.
- Refactor `generate_viz_output_from_graph` so filesystem enrichment is a
  separate input phase and layout is pure.
- Replace `apply_viz_result` with `Arc` publication and bounded stable-ID
  reconciliation.
- Add cancellation checkpoints and cap graph refresh publication at 5 Hz.

### Stage 3: every panel and history

- Convert detail, log/activity/firehose, messages/cursor, monitor/dashboard,
  archive, settings/forms, clipboard, and file browser to keyed panel requests.
- Implement true reverse JSONL paging/tailing; do not read the entire history to
  obtain its tail.
- Bound log suffix reads, directory search batches, preview reads, markdown,
  syntax highlighting, and width-specific projections.
- Remove every loader call from `render::draw` and every storage call from key
  dispatch.

### Stage 4: bounded notification and optional daemon snapshots

- Ship coarse polling first, then add the explicit local nonrecursive watcher
  plan and overflow fallback.
- Add daemon snapshot handshake/fallback using the same schema.
- Keep periodic content-based reconciliation in both modes.

### Stage 5: relocate volatile state

Move the socket, trace, render/index caches, and suitable per-user state to XDG
local storage using dual-read compatibility. This reduces NFS churn but does
not change correctness or the UI-thread contract.

At the end of every stage, a source guard rejects direct `std::fs`, `File`,
`Config::load*`, registry/history loaders, blocking IPC, and `Command::output`
from the TUI constructor, event, and render modules. Exceptions must live in a
named worker/backend module. A debug `UiThreadGuard` marks the render thread;
the test `StorageBackend` panics if called from it.

## Exact validation and benchmark plan

### Deterministic unit tests

Add `src/tui/snapshot_engine/tests.rs` with:

1. `ui_thread_guard_rejects_storage_access` — all production storage entry
   points panic in a guard-marked UI thread; `try_request` is allowed.
2. `request_queue_is_bounded_and_latest_generation_wins` — flood 100,000 graph
   hints; assert queue bounds and only the newest pending generation remains.
3. `interactive_lane_is_not_blocked_by_bulk_read` — hold a graph read for five
   virtual seconds and complete an active-chat request independently.
4. `stale_generation_and_view_key_are_rejected` — complete requests in reverse
   order and after selection/width changes.
5. `mutation_fence_rejects_prewrite_snapshot` — race external refresh, local
   mutation success/failure, and read-after-write reconciliation; preserve the
   editor buffer.
6. `atomic_replace_equal_mtime_and_mtime_rollback_converge` — simulate rename,
   delete/recreate, equal mtimes, stale attributes, and reordered notifications;
   digest/revision rules select the new bytes.
7. `cooperative_cancel_stops_parse_layout_and_projection` — cancellation is
   observed between bounded batches and no result is published.
8. `result_drain_obeys_one_millisecond_budget` and
   `event_drain_obeys_two_millisecond_budget` using an injected clock.
9. `loading_episode_is_thresholded_phase_aware_and_deduplicated` — no flash
   before 150 ms, one slow transition, <=4 Hz phase updates, CPU/storage labels
   differ, error retains stale data, recovery clears and resets the episode.
10. `watch_plan_contains_only_known_nonrecursive_paths` and
    `watch_cap_falls_back_to_polling` — assert no recursive watch under `.wg`,
    cap behavior, overflow fallback, and reconciliation polling.
11. `snapshot_accept_is_arc_swap_and_preserves_interaction_state` — use a 10k
    snapshot and assert no row clone/scan while focus, scroll, dialog, and draft
    survive.
12. `shutdown_does_not_join_stuck_storage_lane` — virtual stuck lane; shutdown
    closes promptly and leaves persistence retryable.

Keep the current `AsyncFs` tests during migration, but replace the ignored
sleep test with deterministic backend tests. Delete the misleading status claim
only after all callers use the new boundary.

### Release benchmarks

Create `benches/tui_snapshot_pipeline.rs` and a fixture generator shared with
integration tests. CI runs the fixed seed and records each phase separately:

```text
cargo bench --bench tui_snapshot_pipeline -- \
  --tasks 10000 --edges 50000 --agents 100 --graph-bytes 26214400
```

Cases are sparse chains, wide fan-out/fan-in, dense layered DAGs (the ancestor
intersection worst case), cycles, 50% filtered/search matches, and 20 refreshes
coalesced during one layout. Measure read, parse, index, cycle/component,
accounting, search/sort, layout, projection, publication, peak memory, and
cancellation latency. The 10k/50k default pipeline must meet the 2-second local
p95 target; every UI acceptance sample remains <=1 ms regardless of fixture.

Extend `tests/integration_tui_perf_benchmarks.rs` with:

- `bench_large_history_tail_reads_bounded_bytes` (100,000 records / 100 MiB,
  assert <=1 MiB read for a page);
- `bench_large_log_tail_reads_bounded_bytes` (100 MiB log plus a 10 MiB burst,
  assert per-result byte/time cap and continuation cursor);
- `bench_file_browser_search_is_incremental_and_cancellable` (100,000 entries,
  bounded batch and latest-query generation);
- `bench_dense_dag_layout_cancellation_and_budget`;
- `bench_snapshot_publication_does_not_scale_with_task_count` (1k versus 10k
  acceptance ratio <=2x and both <=1 ms).

### Real PTY smoke tests

The acceptance gate must drive the human surface, not call the snapshot API.
Add these grow-only scenarios to `tests/smoke/manifest.toml` with owner
`implement-nonblocking-tui` (and the later validation task where applicable):

1. `tests/smoke/scenarios/tui_first_frame_slow_storage.sh` launches the real
   `wg tui` in tmux with the storage backend/shim injecting 1, 3, and 5 seconds
   into metadata, open, read, readdir, and write. It captures the pane directly
   (not waiting for the `.wg` dump socket), asserts the skeleton by 50 ms p99
   over repeated trials, sends focus/text/help keys while reads are outstanding,
   and asserts <=50 ms p99 / <=100 ms max on-screen acknowledgement. It also
   checks one phase-aware slow indicator and its recovery disappearance.
2. `tests/smoke/scenarios/tui_large_graph_continuous_mutation.sh` uses the
   10k/50k fixture, appends/atomically replaces at 20 Hz, switches tabs, types a
   search, opens detail/log/messages/chat, and records frame/input histograms.
   It asserts latest-generation convergence, stable selection/draft, bounded
   RSS, no toast storm, and frame/input budgets.
3. `tests/smoke/scenarios/tui_daemon_snapshot_fallback.sh` runs the same launch
   with daemon running, daemon stopped, daemon killed mid-refresh, incompatible
   handshake, and reconnect. Every mode paints/responds on budget and converges
   to the same graph digest.
4. `tests/smoke/scenarios/tui_large_panels_async.sh` opens a 100 MiB task log,
   100k-message history, 10k-message inbox, 100 active-agent monitor, archive,
   and a 100k-entry file tree through real keystrokes. Page/search changes show
   placeholders/results asynchronously while unrelated input stays on budget.
5. `tests/smoke/scenarios/tui_shutdown_stuck_storage.sh` blocks each I/O lane and
   a persistence write in turn, sends the real quit key/SIGTERM, and asserts
   terminal modes are restored within 100 ms and the process exits within
   250 ms without corrupting durable state.

The latency harness must model NFS-like semantics, not just `sleep` before one
`AsyncFs` operation: per-operation latency, stale metadata, missing/delayed
notifications, short reads, permission/IO errors, atomic rename, delete/recreate,
and reordered completion are independently selectable. A FUSE/sshfs job may be
added as a nightly confirmation, while the injected `StorageBackend` and PTY
shim keep the regular smoke gate deterministic.

Existing human-flow scenarios such as
`tui_chat_pty_last_interaction.sh`, the `tui_log_pane_*` scenarios, and
`viz_tui_preserve_top_anchor.sh` remain regression coverage. They do not replace
the new slow-storage launch and continuous-mutation tests.

### Static acceptance checks

CI additionally runs:

```text
rg 'RecursiveMode::Recursive' src/tui
rg 'std::fs|File::|Config::load|AgentRegistry::load|Command::(new|output)' \
  src/tui/viz_viewer/{mod.rs,event.rs,render.rs,state.rs}
```

The first command must find no `.wg` watcher. Findings from the second must be
zero or mechanically allowlisted bounded terminal-only operations; project
storage and subprocess work must reside in snapshot worker/backend modules.
Render tests instrument work counters and assert `render::draw` touches only
visible rows/columns and never calls a loader.

## Acceptance mapping

- **No filesystem or unbounded graph work on UI:** the thread invariant,
  `StorageBackend` guard, pure constructor/render, CPU pool, bounded event/result
  drains, and static checks make this enforceable.
- **First frame and keys under 1-5 seconds latency:** 50 ms p99 / 100 ms maximum
  contracts and `tui_first_frame_slow_storage.sh` cover the actual PTY path.
- **Large graph and frame budgets:** 10k tasks/50k edges, 25 MiB graph, 16.7/33
  ms frame and 2-second worker refresh targets are explicit.
- **Recursive watching eliminated:** polling-first plus the capped,
  nonrecursive known-path plan contains no recursive `.wg` registration.
- **Restrained feedback:** one thresholded, phase-aware episode slot transitions
  once to slow and clears on recovery.
- **Stale results and mutation races:** per-key generations, revision vectors,
  content identity, cancellation, latest-pending coalescing, mutation fences,
  and stable-ID UI ownership reject stale work explicitly.
- **Required operating modes:** the test matrix covers startup, continuous
  mutation, daemon running/stopped/disconnect, NFS-like behavior, large
  logs/history/messages/file trees, and stuck-storage shutdown.
