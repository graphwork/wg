# Session Log smoke graph-isolation audit

## Regression and boundary

The leaked `smoke-live` task was possible because the original raw-stream smoke changed into a scratch directory and then relied on bare `wg` graph discovery. If the scratch root was nested below a live graph (or worker routing variables survived), `wg init`/`wg add` could resolve the caller graph instead of creating a fixture graph.

The repaired boundary is explicit and redundant:

- the fixture graph is always `$scratch/project/.wg`;
- every WG invocation, including the tmux TUI and `tui-dump`, receives `--dir` (`tests/smoke/scenarios/tui_log_pane_renders_raw_stream.sh:54-72,105-116`);
- HOME, XDG config, and WG global state live below the scenario scratch, and worker/worktree routing variables are removed (`:56-60`);
- the current candidate binary is used consistently rather than mixing a candidate TUI with an installed CLI (`:29-35`);
- helper-owned exact tmux and scratch registrations remain the only cleanup targets.

## Adjacent Session Log audit

| Scenario | Finding | Action |
|---|---|---|
| `tui_log_pane_renders_raw_stream.sh` | Confirmed: bare init/add/TUI/dump plus CWD discovery and inherited HOME | Fixed; this is the primary regression target. |
| `tui_log_pane_follows_retry.sh` | Confirmed same implicit-directory pattern | Fixed at lines 52-70 and 145-177. |
| `tui_log_pane_live_retry_tail.sh` | Confirmed same implicit-directory pattern | Fixed at lines 34-47 and 143-152. |
| `tui_log_scroll_controls.sh` | Confirmed same implicit-directory pattern | Fixed at lines 35-53 and 89-99. |
| `tui_session_log_header_clicks.sh` | Not affected: it already isolated HOME/config, removed inherited routing, and passed explicit `--dir` to every WG process | No change (`:22-27,94`). |

Moving the older fixtures to a truly empty graph also exposed obsolete focus setup: `Escape` or `Ctrl+O` acted on command mode rather than a chat PTY. The fixed scenarios now send `4` directly, matching their synthetic no-chat graph.

## Negative caller sentinel

`tests/smoke/scenarios/tui_log_smoke_graph_isolation.sh` creates a real, short-path caller graph and runs the primary scenario from `caller/nested/deeper`, with the target smoke root physically below that caller. It deliberately injects inherited `WG_DIR`, project, task, agent, HOME, and config values.

After each of four exits—full pass, forced post-init failure, TERM while a real tmux TUI is owned, and pre-fixture missing-tmux skip—the meta-scenario proves:

1. caller `graph.jsonl` SHA-256 is byte-identical;
2. parsed caller task count is unchanged and the negative sentinel remains;
3. caller `service/registry.json` SHA-256 is byte-identical;
4. no `smoke-live`, `.flip-*`, or `.evaluate-*` task reached the caller;
5. no target scratch child, helper-owned tmux session, or process with the target graph prefix remains.

The assertions are centralized at `tui_log_smoke_graph_isolation.sh:60-99` and invoked for pass/failure/signal/skip at lines 120, 140, 170, and 188.
