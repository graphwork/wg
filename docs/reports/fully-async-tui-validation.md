# Fully asynchronous TUI validation

Date: 2026-07-14
Task: `validate-fully-async`
Result: **FAIL — the first-frame and large-graph lanes pass, but the complete
asynchronous contract is not yet satisfied.**

## Scope and revisions

The code under test started at
`e3e6f72a2c0e82ab97bf8efbc707d403dda54cff` (`move-large-graph`), whose direct
TUI predecessors are
`e70bb92a7079c10d157caa9db8cd6357ac247978` (`implement-nonblocking-tui`) and
`0a4665f1e7443fb1ae8abe5f4d157a0768ef4fa1`
(`design-fully-asynchronous`). Measurements below were repeated after the
Config-render correction in
`b2cab06e216275826f7dd165b437f6999f6fc74e`. The host was Linux x86-64,
Rust 1.96.0, tmux at 120x40 (160x50 for large-log panels), and `/dev/md2`
local storage. The deterministic shim converted that local storage into a
network-filesystem-like path by delaying selected libc `stat`/`statx`/
`fstatat`, `open`/`openat`, first `read`, and `rename` calls and by returning
`ENOSPC` from `inotify_add_watch`.

The task also removes two direct Config-render reads: configuration is rebuilt
on a dedicated, coalesced one-result lane; render consumes the cached endpoint
map and shows a placeholder before it arrives. This is a real reduction in the
reachable blocking surface, but the broader refresh path described below still
prevents a PASS verdict.

## Commands

The validation used the checked-in scenarios plus deterministic PTY harnesses.
Build commands were run with `CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0` after an
earlier parallel attempt exhausted the host through unrelated leaked `/tmp`
Cargo targets.

```text
wg quickstart
wg agent-guide
wg show validate-fully-async

cargo fmt
cargo fmt --check
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo clippy --locked
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test --locked
# The managed target was reaped twice after the unit/binary suites; the clean
# full run used a target outside the worktree and removed worker-only service
# control variables expected to be absent by integration_chat.
env -u WG_TASK_ID -u WG_AGENT_ID \
  CARGO_TARGET_DIR=/home/bot/wg/.validate-target-agent-389 \
  CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo test --locked

bash tests/smoke/scenarios/tui_responsive_under_500ms_latency.sh
bash tests/smoke/scenarios/tui_first_frame_slow_storage.sh
bash tests/smoke/scenarios/tui_large_graph_continuous_mutation.sh

# Five trials at each delay; launch wg tui in tmux, capture the neutral frame,
# press ?, observe Navigation, press q, and time process disappearance.
WG_TUI_TEST_STORAGE_LATENCY_MS={1000,3000,5000} wg tui --no-mouse --show-keys

# LD_PRELOAD matrix, matching the fixture's .wg path only.
WG_FS_SHIM_OPS={stat,open,read} WG_FS_SHIM_LATENCY_MS=50 wg tui ...
WG_FS_SHIM_FAIL_WATCH=1 wg tui ...
WG_FS_SHIM_OPS=rename WG_FS_SHIM_LATENCY_MS=500 mv graph.next graph.jsonl

# Daemon/new-graph modes.
wg tui ...                                  # absent .wg / newly initialized
wg service start --no-chat-agent; wg tui ...
wg service stop --force; wg tui ...

# Recovery and feedback.
printf '{corrupt\n' >.wg/graph.jsonl
WG_TUI_TEST_STORAGE_LATENCY_MS=1000 wg tui ...
mv valid-graph .wg/graph.jsonl
grep -Rci 'Storage slow' .wg/service

# Scale fixtures, driven through tmux.
# - 10,000 tasks / about 49,995 edges; 20 atomic mutations at 20 Hz
# - 100,000 history records / 97,150,000 bytes
# - 105,000 valid assistant NDJSON records / 101,955,000 bytes

# Static acceptance checks from docs/design-fully-async-tui.md.
rg 'RecursiveMode::Recursive' src/tui
rg 'std::fs|File::|Config::load|AgentRegistry::load|Command::(new|output)' \
  src/tui/viz_viewer/{mod.rs,event.rs,render.rs,state.rs}

cargo install --path . --locked
```

## Measurements against the design budgets

All latency values include tmux command/capture overhead, so they are
conservative user-visible measurements. Percentiles use nearest-rank.

| Surface | Budget | Result | Verdict |
| --- | ---: | ---: | --- |
| First frame, 1 s storage delay (n=5) | p99 <= 50 ms; dispatch p99 <= 100 ms | p50 23 ms, p99/max 33 ms | PASS |
| First frame, 3 s delay (n=5) | same | p50 23 ms, p99/max 24 ms | PASS |
| First frame, 5 s delay (n=5) | same | p50 31 ms, p99/max 32 ms | PASS |
| Help acknowledgement during 1/3/5 s bootstrap delay | p99 <= 50 ms, max <= 100 ms | p99/max 18/9/18 ms | PASS |
| Shutdown during 1/3/5 s stuck bootstrap read | restore <= 100 ms; exit <= 250 ms | worst observed exit 25 ms | PASS |
| Continuous 10k/50k mutation, 50 presentation samples | input p99 <= 50 ms, max <= 100 ms | p50 14 ms, p95/p99/max 16 ms | PASS |
| Render frame time | p95 <= 16.7 ms, p99 <= 33 ms | no internal render histogram exists; presentation-cycle surrogate p95/p99 16/16 ms | **FAIL (instrumentation gap)** |
| Input dispatch / result acceptance | <= 2 ms / <= 1 ms | no internal phase counters exported | **FAIL (instrumentation gap)** |
| 10k/50k local publication | p95 <= 2 s in release | installed release run 628 ms; an earlier debug smoke run at 2,700 ms is non-comparable | PASS on required release profile |
| Latest-generation convergence | no stale rollback | 20th generation appeared and remained stable in the installed release smoke | PASS |
| Large-graph memory | <= two graph generations plus bounded projections; three-owner transient <= 1 GiB | installed release run: 290,240 -> 502,992 KiB RSS, 811,492 KiB peak | PASS |
| 100k chat records / 97,150,000 bytes | first/input targets; bounded retained page | first 41 ms, input 9 ms, load 294 ms, 61,436 KiB steady, 155,208 KiB peak | PASS for launch/input/memory |
| Valid active log / 101,955,000 bytes | base graph <= 2 s; page <= 1 MiB/200 records | graph did not publish within 82,708 ms | **FAIL** |
| Config open/read latency, 500 ms | help p99 <= 50 ms, max <= 100 ms | Config content 15,247 ms; help 15,219 ms | **FAIL** |
| Stat/open/read matrix, 50 ms per matched operation | first/input <= 100 ms | stat 117/6 ms; open 71/7 ms; read 26/8 ms | stat first frame **FAIL** by 17 ms; all input PASS |
| Atomic rename plus unavailable watcher | eventual polling convergence | delayed rename 509 ms; query-visible convergence 7 ms after rename | PASS |
| Corrupt graph then restore | compact failure, eventual recovery | first 14 ms, input 9 ms, recovery 2,904 ms | PASS |

The frame surrogate measures key-to-present rather than `render::draw` itself.
It is useful evidence that ordinary frames fit the user-visible budget, but it
cannot honestly satisfy the design's phase-level frame/dispatch/acceptance
requirement. The missing counters are therefore recorded as a failure, not
inferred as a pass.

## Queue-depth and generation audit

The graph pipeline is bounded and latest-wins:

- `AsyncFs`: bulk request capacity 8, interactive request capacity 32,
  response capacity 64, with per-key in-flight deduplication.
- `BootstrapEngine`: request capacity 1, result capacity 4, exactly one newest
  local pending request, one fixed worker, stale-generation rejection, and no
  join on drop.
- `SnapshotEngine`: request/result/retire capacities 1/1/1, one newest local
  pending build and one pending retirement. Its 100,000-request unit test
  asserts `pending_len() <= 1` and generation 100,000 wins.
- Config correction in this task: one in-flight rebuild and a one-result
  channel; repeated reload hints coalesce.

Thus graph and Config queue growth is bounded. This does not rescue the full
contract: legacy auxiliary loaders are still called synchronously, and a
single huge log generation has unbounded work even though the number of queued
generations is bounded.

## Static reachability audit

`rg 'RecursiveMode::Recursive' src/tui` returned no match, so watcher-limit
fallback no longer depends on a recursive `.wg` watch. The watcher-failure PTY
run also converged through polling after an atomic replacement.

The required blocking-call scan did **not** pass. Excluding test-only code, it
still finds project-storage operations reachable from the terminal loop,
including:

- `render.rs::build_coordinator_runtime_lines`: `Config::load_or_default`,
  compactor-state/inbox/outbox reads, and an existence check during draw;
- `render::draw`/`draw_right_panel`: lazy detail/log/messages/agency/
  coordinator-log/activity/settings loaders and log updates;
- `state.rs::maybe_refresh`: coordinator/chat polling, service/vitals/time
  refresh, detail/log/message/activity updates, registries, metadata and log
  reads;
- input handlers that directly call detail/log/config/settings/archive and
  clipboard/subprocess paths;
- log/history helpers that read or wrap complete files rather than a bounded
  byte/record page.

The scan produced 101 raw matches below line 20,000 alone (some are legitimate
worker-only helpers, so raw count is not a call-graph count). Dynamic injection
proves at least the periodic Config/chat/service path is reachable: while the
main thread was delayed, `ps -T` showed the terminal thread in
`hrtimer_nanosleep` and help did not render for about 15.2 seconds. Therefore
the required claim “no synchronous filesystem call or unbounded derivation is
reachable from input/render” is false at this revision.

## Slow-storage feedback and recovery

The 1/3/5-second bootstrap scenario displayed one compact
`Storage slow ... discover` slot after the 150 ms threshold and cleared it
after publication. The corrupt-then-restore run found one slot during failure,
zero after recovery, and zero `Storage slow` matches under `.wg/service`.
There was no coordinator/service log spam. This portion passes.

## Operating-mode ledger

| Case | Result | Notes |
| --- | --- | --- |
| `.wg` absent / new graph | PASS | first 14 ms, help 7 ms |
| Empty graph, daemon stopped | PASS | first 29 ms, help 9 ms |
| Empty graph, daemon running | PASS | first 13 ms, help 7 ms |
| Daemon stopped after run | PASS | clean shutdown; graph correctness independent of daemon |
| Continuous atomic graph mutation | PASS | bounded latest-wins graph lane |
| Watch registration unavailable | PASS | polling observed replacement |
| Slow bootstrap storage | PASS | neutral shell, input and shutdown independent of worker |
| Failed/corrupt storage and recovery | PASS | compact feedback cleared after restore |
| Large chat history | PASS | launch/input/memory stayed bounded |
| Large active log tree | FAIL | log enrichment held back base publication |
| Slow auxiliary/config storage | FAIL | periodic UI-thread refresh blocked input |
| SIGTERM-specific stuck persistence lane | NOT PROVEN | quit-key stuck-worker case passes; no separate persistence-lane injector exists |

## Compatibility notes and disposition

The first-frame shell, graph-generation fencing, continuous-mutation behavior,
nonrecursive watcher fallback, and shutdown-with-stuck-bootstrap behavior are
compatible with daemon-running and daemon-stopped operation. The Config change
is wire/config compatible: it changes only where an existing panel snapshot is
built and preserves endpoint test-result and UI selection state on install.

No release should describe the TUI as “fully asynchronous” yet. Two follow-up
tasks were added with live-PTY acceptance criteria:
`finish-aux-tui-snapshot-lanes` for auxiliary render/refresh/input storage, and
`bound-async-tui-log-enrichment` for bounded log/token pages and base-graph
publication. The installed binary includes the validated first-frame/graph and
Config-lane improvements, but this report deliberately retains the overall
FAIL verdict until those tasks and the missing phase telemetry land.

The ordinary aggregate test command passed the library suite (2,845/2,845) and
binary suite (3,730/3,730, one ignored) twice, but on both attempts the running
coordinator reaped the managed `target/` before Cargo could launch
`agency_schema_fields`. That target passed directly (5/5). The authoritative
full rerun used the external target shown above; removing the inherited worker
identity was also necessary because `integration_chat` intentionally starts and
stops an isolated service, an operation production workers correctly refuse.
That rerun passed every integration executable and all doctests.

## Validation gate ledger

| Gate | Result |
| --- | --- |
| `cargo fmt` | PASS |
| `cargo fmt --check` | PASS |
| `cargo clippy --locked` | PASS (warnings only) |
| full `cargo test --locked` | PASS in isolated target with worker-only `WG_TASK_ID`/`WG_AGENT_ID` removed; all integration binaries and doctests passed |
| `tui_responsive_under_500ms_latency` | PASS |
| `tui_first_frame_slow_storage` | PASS |
| owned `tui_large_graph_continuous_mutation` | PASS: load 628 ms, key 10 ms, RSS 290,240 -> 502,992 KiB, peak 811,492 KiB |
| `cargo install --path . --locked` | PASS |
