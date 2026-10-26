# Auxiliary TUI snapshot-lane audit

Date: 2026-07-14
Task: `finish-aux-tui-snapshot-lanes`

## Result

The TUI input/render loop no longer performs auxiliary panel refreshes or lazy
panel loads. It submits bounded, non-blocking requests to one persistent
storage worker and installs completed snapshots with non-blocking polling.
This covers config, settings, detail, active-task log, messages, agency,
coordinator log/runtime state, dashboard/agent monitor, firehose, output,
chat/outbox/history, service/vitals/time, history browser, chat manager, file
browser, and tab-state persistence.

The live human-flow gate passed with a worst observed help acknowledgement of
11 ms across all nine reachable inspector tabs while an `LD_PRELOAD` shim
delayed matching `stat`, `open`, and first `read` calls by 500 ms each. The
contract is below 100 ms.

## Execution boundary

`src/tui/viz_viewer/auxiliary.rs` defines the boundary:

- `Lane::request` uses `SyncSender::try_send`; it never waits.
- The work queue and completion queue each hold one item.
- `Kind`-keyed pending requests are coalesced, preventing repeated renders or
  filesystem event bursts from growing a backlog.
- The `wg-tui-aux-storage` thread alone executes storage closures.
- `Lane::drain` uses `Receiver::try_recv`; it never waits.

`VizApp::poll_auxiliary_snapshots` is called at the start of
`VizApp::maybe_refresh`. Completion closures contain only snapshot installation
and stale-result checks. They do not perform the load a second time.

The render path contains only demand signals. Empty panels request a snapshot
and continue painting their cached/placeholder state. In particular, Log and
Messages do not submit unconditional refreshes on every frame; a graph or file
change first invalidates the cache, and the next request repopulates it.

## Lane inventory

| Snapshot kind | Storage/derivation performed on worker | Publication guard |
|---|---|---|
| Config | local/global/profile config load and derived entries | current app, one in-flight kind |
| Settings | profile/config inspection | panel snapshot replacement |
| Detail | task detail, iteration archive and output metadata | selected task anchor |
| Log | attempt selection, output and stream events | selected task anchor; interactive scroll modes preserved |
| Messages | coordinator message files and summary | selected task anchor |
| Agency | lifecycle/event derivation | selected task anchor |
| CoordinatorLog | daemon/operations log plus coordinator runtime files | active coordinator anchor |
| Firehose / Output | filesystem-backed feed/output panels | selected task anchor where applicable |
| AgentMonitor | registry/dashboard snapshot | cached snapshot replacement |
| Chat | outbox polling and service-response state | active coordinator and cursor/version guards |
| Service | health, vitals, coordinator activity, time/status state | cached snapshot replacement |
| ChatHistory | initial, paginated, and full history loads | active coordinator and starting-length guards |
| HistoryBrowser / ChatManager / FileBrowser | lazy modal contents | modal-open/current-directory guards |
| Persistence | `tui-state.json` write | serialized/coalesced best-effort job |

## Static call-graph audit

The permanent smoke scenario extracts the production portions of
`event.rs` and `render.rs` (excluding their `#[cfg(test)]` modules) and rejects:

- direct `std::fs`, `File::open`, `read_to_string`, `read_dir`, metadata/stat,
  config-load, and `std::process::Command` operations;
- calls to the legacy synchronous detail/log/messages/agency/coordinator-log/
  settings/service/chat loader methods.

Manual tracing then followed every `request_*` call in those two files to the
request methods in `state.rs`, from each request method to
`Lane::request`, and from completion publication to
`poll_auxiliary_snapshots`. The only channel operations on the terminal side
are `try_send` and `try_recv`. The worker uses blocking `recv`/`send`, but those
operations are confined to `wg-tui-aux-storage` and both channels are bounded.

Unbounded panel derivations run inside the worker closures, not inside their
completion closures. Render-time work is limited to formatting cached panel
snapshots and bounded viewport content. User-triggered WG commands continue to
use the existing asynchronous command runner rather than a direct subprocess
from `event.rs` or `render.rs`.

## Permanent validation

`tests/smoke/scenarios/tui_auxiliary_snapshot_latency.sh` performs the real
terminal flow:

1. Create a representative graph and start the installed `wg tui` in tmux.
2. Wait for the initial graph snapshot so the test isolates auxiliary lanes.
3. Enable a runtime-controlled preload shim that delays matching project
   storage `stat`, `open`, and first `read` calls by 500 ms.
4. Visit inspector tabs 0 through 8, explicitly force Detail, Config, and
   Settings refresh controls, press `?`, and measure when the Navigation help
   overlay becomes visible.
5. Fail if any acknowledgement is 100 ms or slower.
6. Prove all three syscall classes were observed on named worker threads and
   fail if any delayed project-storage call is attributed to the TUI main
   thread, then run the static audit.

The scenario is listed in the grow-only smoke manifest with
`owners = ["finish-aux-tui-snapshot-lanes"]`, so `wg done` runs it as this
task's hard smoke gate.

## Focused regression tests

- `request_is_coalesced_and_never_waits_for_slow_work` holds the worker for
  500 ms and verifies request submission remains below 50 ms, duplicate kinds
  coalesce, and completion drains correctly.
- `config_panel_load_runs_on_auxiliary_lane` verifies two rapid Config requests
  produce one in-flight job and publish through the completion poll.
