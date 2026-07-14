# Fully asynchronous TUI final revalidation

Date: 2026-07-14

Task: `revalidate-fully-async`

Result: **FAIL — non-provisional.**

The two blockers from the discovery run are fixed: auxiliary panels no longer
perform storage work on the terminal thread, and a valid 100 MiB-class active
agent log no longer holds back base-graph publication. The final acceptance
gate nevertheless found one different, reachable contract violation. A valid
`[tui] chat_page_size = 100000` setting publishes 100,000 history records to
the live Chat view. With a 97,100,000-byte valid history, the neutral shell
appeared in 15 ms, but after the history snapshot arrived the Help key was
still not acknowledged after 4,648 ms. RSS increased from 160,268 to 348,044
KiB. The setting and the CLI history-depth override are not capped before
render/search work, so the required invariant that input/render derivation is
bounded is false at the tested revision.

This report is the terminal ledger for this revision, not a provisional pass.
WG must not describe the TUI as fully asynchronous until the history-page
bound is enforced and the failing human-flow probe passes.

## Revisions under test

The installed binary, PTY tests, static audit, and Rust gates all used the same
acceptance revision.

| Purpose | Exact commit |
| --- | --- |
| Acceptance revision (`main`/starting HEAD) | `2f742c9f4cf229bf1390cf6265ee6ffa69b6dc21` |
| Bounded active-log fix | `0a5a1e341646f2c9e159205ed41d370042878d7c` |
| Auxiliary snapshot-lane integration on `main` | `0512f6fe8461d3a5d326f441834cfd0f4cf05fd1` |
| Auxiliary lane implementation checkpoints | `e08b52387237b6eb0cb62b4670e35a3259eba8ae`, `125b9c20f46ccc3101af4e90cd193f3482a11ec9`, `edd3cce21d1502886e6b13e90cf0b3057525c205` |
| Discovery validation | `e3644c13` |
| Large-graph / nonblocking / design ancestry | `e3e6f72a`, `e70bb92a`, `0a4665f1` |

The report and acceptance-harness changes are committed separately after the
measurements; that validation-only commit does not change the production
binary under test.

Host: Linux x86-64, Rust 1.96.0, tmux PTYs. Latency injection used the existing
test shim for matched `stat`/`statx`/`fstatat`, `open`/`openat`, and first
`read` calls, plus the bootstrap test delay and watcher-failure injection.

## Acceptance budgets

The normative budgets are from `docs/design-fully-async-tui.md`:

- first frame: p99 at most 50 ms (dispatch allowance at most 100 ms);
- input acknowledgement: p99 at most 50 ms and never 100 ms or more;
- base-graph publication: at most 2 seconds for the scale fixture;
- shutdown: terminal restoration at most 100 ms and process exit at most
  250 ms;
- scale: 10,000 tasks, approximately 50,000 dependency edges, 100 active
  agents, a 100 MiB-class active log, and 100,000 history records must not
  change the first-frame or input budgets;
- all storage reads must be worker-owned and all work applied to the terminal
  thread must be bounded by a viewport/page projection.

All timings below are end-to-end tmux observations and therefore include PTY
command and capture overhead. Percentiles use nearest rank.

## Injected-latency matrix

### Bootstrap storage, first frame, Help, feedback, and shutdown

Five independent PTY trials were run at every bootstrap delay. Every trial
showed the neutral shell before storage completed, accepted `?`, displayed one
compact slow-storage slot after the threshold, cleared that slot after a
snapshot was published, and exited while the worker could still be delayed.

| Injected delay | First-frame samples (ms) | Help samples (ms) | Quit samples (ms) | Verdict |
| ---: | --- | --- | --- | --- |
| 1,000 ms | 13, 13, 28, 14, 14 | 8, 9, 7, 8, 8 | 21, 20, 20, 21, 20 | PASS |
| 3,000 ms | 13, 27, 27, 14, 26 | 9, 9, 8, 9, 8 | 21, 20, 21, 20, 21 | PASS |
| 5,000 ms | 28, 27, 13, 26, 14 | 8, 9, 7, 9, 8 | 20, 21, 22, 20, 21 | PASS |
| All 15 | p50 14; p99/max 28 | p50 8; p99/max 9 | p50 21; p99/max 22 | PASS |

The permanent `tui_first_frame_slow_storage.sh` scenario now reports all three
measurements and enforces the design's strict `<250 ms` process-exit budget,
instead of only checking that the session disappeared within 500 ms.

### Auxiliary Config and inspector panels

`tui_auxiliary_snapshot_latency.sh` visited every reachable inspector tab and
opened Help immediately while its worker lane was stalled. The nine-tab flow
includes the Config/Settings panel (`r`), Detail (`R`), and the other auxiliary
views. All three requested syscall classes were observed in the shim log; the
test also scans the production portions of `event.rs` and `render.rs` for
direct storage/config/process access and legacy synchronous loader calls.

| `stat`/`open`/first-`read` delay | Maximum Help acknowledgement | Verdict |
| ---: | ---: | --- |
| 500 ms | 9 ms | PASS |
| 1,000 ms | 9 ms | PASS |
| 3,000 ms | 9 ms | PASS |
| 5,000 ms | 8 ms | PASS |

The scenario is parameterized by `WG_TUI_AUX_LATENCY_MS`, so the full matrix is
repeatable rather than being a one-off harness modification.

## Scale, enrichment, and publication

| Fixture / behavior | Measurement | Verdict |
| --- | --- | --- |
| 10,000 tasks / about 49,995 edges; 20 atomic replacements at 20 Hz | base publication 629 ms; Help 10 ms; RSS 289,548 -> 649,592 KiB; peak 801,624 KiB; generation 20 remained stable | PASS |
| Valid active-agent `output.log`, exactly 101,955,000 bytes | first frame 24 ms; Help 9 ms; base graph 445 ms | PASS |
| Valid 100,000-record history, 97,100,000 bytes, bounded page (`chat_page_size=100`) | first 22 ms; Help 9 ms; graph 253 ms; steady RSS 25,796 KiB; quit 118 ms | PASS |
| Same valid history with valid `chat_page_size=100000` | first 15 ms; Help not acknowledged after 4,648 ms (`ack=0`); RSS 160,268 -> 348,044 KiB | **FAIL** |

The active-log result is the direct proof that enrichment cannot starve base
graph publication: the exact valid log that blocked the discovery revision for
more than 82 seconds now leaves publication at 445 ms, below the 2-second
budget. Unit tests additionally pin the active-log projection at 1 MiB / 200
records and preserve terminal-result precedence.

The graph mutation run proves bounded latest-generation behavior under churn:
all 20 replacements were observed through the asynchronous path, no older
generation rolled the UI back, and interactive acknowledgement stayed at
10 ms while publication was active.

## Operating-mode and recovery ledger

| Case | Exact observation | Verdict |
| --- | --- | --- |
| Daemon absent/stopped | first 21 ms; Help 8 ms; graph 40 ms; quit 15 ms | PASS |
| Daemon running | first 24 ms; Help 8 ms; graph 43 ms; quit 16 ms | PASS |
| Daemon stopped after running | first 16 ms; Help 9 ms; graph 37 ms; quit 15 ms | PASS |
| Watch registration unavailable | atomic-replacement polling converged without recursive watch support | PASS |
| Corrupt graph under 1,000 ms storage delay | first 21 ms; Help 7 ms; one `Storage slow · discover` slot | PASS |
| Atomic repair of corrupt graph | valid replacement visible 754 ms after rename; feedback cleared | PASS |
| Slow/corrupt storage warning spam | zero matching warnings in service logs | PASS |
| Daemon-running warning-spam hold (2.2 s) | zero slow-storage/watcher/TUI warning matches | PASS |
| Quit during 1--5 s delayed worker | worst process exit 22 ms | PASS |
| Mutation storm | 20 atomic writes at 20 Hz; latest generation stable | PASS |

`rg 'RecursiveMode::Recursive' src/tui` returned no match. Fallback
correctness was exercised, rather than inferred, by disabling watch
registration and observing atomic graph replacement through polling.

Feedback behaved as a single state slot, not a log stream: it appeared once
after the 150 ms threshold, changed with the active phase, cleared after
successful publication/recovery, and produced no service/coordinator warning
spam.

## Static input/render reachability audit

The original synchronous-storage blocker is fixed. The permanent auxiliary
scenario extracts production code before `#[cfg(test)]` and rejects direct
filesystem/process/config-load calls from `event.rs` and `render.rs`, as well
as calls to the retired synchronous panel loaders. It passed under every
latency in the 500 ms--5 s matrix. Auxiliary lane request/result queues are
bounded 1/1, requests use nonblocking `try_send`, results use `try_recv`, and
work coalesces by snapshot kind. The graph path is likewise generation-fenced
and latest-wins.

The broader required assertion still fails because derivation is unbounded:

- `Config.tui.chat_page_size` is an unconstrained `usize` at
  `src/config.rs:838-839`;
- `load_chat_history` and `load_chat_history_for_coordinator` pass either the
  CLI override or that setting through as the requested page size at
  `src/tui/viz_viewer/state.rs:15057` and `:15088` (with another config use at
  `:15296`);
- the worker can therefore publish 100,000 messages to the app;
- live Chat drawing wraps/renders that loaded collection, and chat search
  scans it on the interactive path.

This is not a theoretical grep finding. The PTY probe used a valid graph,
valid history, and valid public configuration, waited for the snapshot, then
sent the real Help key through tmux. The terminal did not acknowledge it in
the 4,648 ms observation window. Consequently, the requirement “no
synchronous storage access or unbounded derivation is reachable on
input/render paths” is **FAIL** even though the synchronous-storage half now
passes.

## Commands and reproducibility

```text
wg quickstart
wg agent-guide
wg show revalidate-fully-async

cargo fmt
cargo fmt --check
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo build --locked
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo clippy --locked

# Authoritative full run. The trap removed the external target on success,
# failure, SIGINT, or SIGTERM; worker service-control variables were removed.
target=/home/bot/wg/.revalidate-target-agent-402
trap 'rm -rf "$target"' EXIT INT TERM
env -u WG_TASK_ID -u WG_AGENT_ID \
  CARGO_TARGET_DIR="$target" CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 \
  cargo test --locked

CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 cargo install --path . --locked

bash tests/smoke/scenarios/tui_first_frame_slow_storage.sh
WG_TUI_AUX_LATENCY_MS=500  bash tests/smoke/scenarios/tui_auxiliary_snapshot_latency.sh
WG_TUI_AUX_LATENCY_MS=1000 bash tests/smoke/scenarios/tui_auxiliary_snapshot_latency.sh
WG_TUI_AUX_LATENCY_MS=3000 bash tests/smoke/scenarios/tui_auxiliary_snapshot_latency.sh
WG_TUI_AUX_LATENCY_MS=5000 bash tests/smoke/scenarios/tui_auxiliary_snapshot_latency.sh
bash tests/smoke/scenarios/tui_large_active_log_enrichment.sh
bash tests/smoke/scenarios/tui_large_graph_continuous_mutation.sh

rg 'RecursiveMode::Recursive' src/tui
```

The daemon/recovery/history probes used isolated temporary HOME and graph
directories and drove `wg tui --no-mouse --show-keys` via tmux. The daemon
fixture was paused before startup, so no model call was made.

## Build, test, install, and cleanup ledger

| Gate | Result |
| --- | --- |
| `cargo fmt` | PASS |
| `cargo fmt --check` | PASS |
| `cargo build --locked` | PASS in 4m14s; warnings only |
| `cargo clippy --locked` | PASS in 2m09s; warnings only |
| Full `cargo test --locked` | PASS: all unit, binary, integration, and doctest executables completed; expected credential/live tests ignored |
| Installed-release PTY bootstrap scenario | PASS |
| Installed-release auxiliary 500 ms--5 s matrix | PASS |
| Installed-release exact active-log scenario | PASS |
| Owned installed-release 10k-task/50k-edge mutation scenario | PASS |
| `cargo install --path . --locked` | PASS in 11m22s; max RSS 2,454,524 KiB; installed `wg` and `nex` from agent-402/acceptance HEAD |

The first managed-target `cargo test` attempt passed its library and binary
suites, then the coordinator reaped a managed executable before Cargo could
launch it. That environmental race is not counted as acceptance. The complete
authoritative rerun used the bounded external target above, exited zero, and
finished all doctests. At completion:

- `/home/bot/wg/.revalidate-target-agent-402` did not exist (trap cleanup
  verified explicitly);
- no other external `CARGO_TARGET_DIR` was created by this task;
- the inherited `/home/bot/wg/.wg-worktrees/agent-402/target` is WG-managed
  shared build storage inside the assigned worktree, not an external target;
- all PTY scratch directories and tmux sessions were removed by their scenario
  traps, including failure paths.

## Final disposition

The original auxiliary-storage and active-log blockers are closed, and every
other requested functional lane passed: first frame, Help, base publication,
daemon on/off, fallback polling, corruption/recovery, mutation storm,
10k-task/50k-edge scale, bounded large history, valid large log, shutdown,
feedback clearing, and warning-spam suppression.

The release gate remains **FAIL** because a public, valid configuration reaches
unbounded Chat render/search work and causes an input miss two orders of
magnitude above the 100 ms hard maximum. The required remediation is to apply
one hard page projection to every history source (configuration, CLI override,
coordinator fallback, pagination/search), bounded to at most 200 records and
1 MiB before publication, then preserve the failing 100,000-record tmux flow
as a permanent smoke scenario and rerun this ledger. The focused follow-up is
WG task `bound-tui-chat`.
