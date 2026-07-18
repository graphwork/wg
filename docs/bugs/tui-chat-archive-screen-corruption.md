# TUI chat Archive screen corruption

## Symptom

On installed commit `5c245a70`, **Close… → Archive** successfully changed the
chat task to `Done` + `archived`, but the outer `wg tui` frame became physically
corrupt. Graph text was overlaid, the active-chat header could coexist with
stale content, and borders appeared displaced.

This was not graph-state corruption. It was a physical-terminal / ratatui-buffer
divergence.

## Credential-free reproduction

The reproducer used the installed `wg` binary at `5c245a70` and the real human
flow in a 120×36 tmux outer session:

```text
wg init --no-agency
wg chat new --name cancel  --command cat
wg chat new --name stop    --command cat
wg chat new --name archive --command cat
tmux new-session -d -s outer -x 120 -y 36 'wg tui'
# drive Ctrl+O, w, a, y with `tmux send-keys`
```

The complete permanent flow is
[`tests/smoke/scenarios/tui_chat_close_lifecycle.sh`](../../tests/smoke/scenarios/tui_chat_close_lifecycle.sh).
It exercises Stop plus Archive's two-stage present-session → already-gone cleanup. During
the investigation, `tmux pipe-pane -O` recorded every byte written to the outer
PTY and `tmux capture-pane -p` recorded the physical pane before and after the
Archive confirmation.

Timeline from the installed-binary run (Unix seconds):

```text
1784284779.479066736  sent Archive confirmation (`y`)
                      background `wg chat archive` killed chat-2's tmux session
                      ArchiveCoordinator drained and attempted the duplicate kill
1784284780.025702600  captured physical outer pane after completion
```

The outer PTY recording contained the following contiguous bytes (error starts
at byte offset 2038 of the completion recording):

```text
\x1b[49m\x1b[59m\x1b[0m\x1b[?25l
can't find session: wg-chat-adhoc-gDHGld-chat-2\x0a
\x1b[2;1H\x1b[38;5;6;48;2;186;186;74m.chat-1...
```

Hex around the diagnostic:

```text
1b 5b 3f 32 35 6c
63 61 6e 27 74 20 66 69 6e 64 20 73 65 73 73 69 6f 6e 3a 20
77 67 2d 63 68 61 74 2d 61 64 68 6f 63 2d 67 44 48 47 6c 64
2d 63 68 61 74 2d 32 0a
1b 5b 32 3b 31 48
```

A direct control command confirmed tmux 3.4's behavior:

```text
$ tmux kill-session -t wg-missing-proof
can't find session: wg-missing-proof
# exit 1; stderr bytes are exactly the printable message above plus LF
```

The captured physical row after Archive was visibly overwritten:

```text
.chat-1can't find sess4on: wg-chat-adhoc-gDHGld-chat-2
```

That ordering is decisive: the tmux diagnostic arrived outside ratatui, moved
the physical cursor, then ratatui emitted a differential update beginning with
`CSI 2;1 H`. Ratatui's previous buffer did not contain the diagnostic or cursor
movement, so unchanged cells were not repainted.

## Root cause

There were two cleanup stages:

1. The captured asynchronous subprocess ran `wg chat archive`. Its
   `chat_cmd::run_archive` path killed the canonical chat tmux session before
   returning.
2. `VizApp::drain_commands` received `CommandEffect::ArchiveCoordinator`,
   removed the local `PtyPane`, and called `PtyPane::kill_underlying_session`
   again.

At `5c245a70`, the second call used
`tmux kill-session ... .status()` with inherited stdout/stderr. Because stage 1
had already removed the session, tmux wrote its missing-session error directly
to the alternate screen.

The adjacent Stop path could reach the same local pane helper. Other tmux IPC
used for scroll/top/bottom/copy-mode and initial `new-session` also used
inherited output. Most normally succeed silently, but any race or missing
session could produce the same class of corruption.

## Fix and invariant

`src/tui/pty_pane.rs` now centralizes host-side tmux `.status()` execution in a
helper that sends both stdout and stderr to `Stdio::null()`. Every runtime tmux
status call uses that boundary. Calls that use `.output()` already capture both
streams. Embedded vendor/chat processes remain intentionally connected to their
owned PTY and vt100 parser; only host-side helpers are terminal-silent.

`tmux_kill_session` is now doubly idempotent:

- it probes `has-session` before issuing a redundant kill, so the normal Archive
  completion does not invoke a second `kill-session`; and
- the kill itself is silenced, so a probe/kill race still cannot touch the outer
  terminal.

The canonical chat-id cleanup helper in `src/chat_id.rs` also nulls both output
streams. Therefore Archive with a local pane, Archive after the session is
already gone, Archive without a local pane, and adjacent Stop cleanup all share
the no-inherited-output invariant.

No continuous or completion-time full clear was added. Once every host-side
subprocess output is captured or nulled, no untracked bytes or cursor movement
exist at the command-completion boundary; the normal single differential redraw
remains synchronized and preserves the fully asynchronous render/input loop.

## Regression coverage

- `already_gone_lifecycle_cleanup_inherits_no_output` runs the missing-session
  cleanup in a subprocess and asserts its inherited stderr is empty. Its control
  command first proves the same tmux installation emits the diagnostic for an
  unsilenced duplicate kill.
- `archive_completion_without_local_pane_is_idempotent` exercises the successful
  `ArchiveCoordinator` completion path with no local pane/session.
- `tui_chat_close_lifecycle` drives real installed-binary keystrokes and covers
  Stop plus Archive's present-session CLI kill followed by its already-gone
  TUI completion cleanup. After each Archive it asserts:
  - every sentinel graph row appears exactly once;
  - the active `.chat-0` identity header appears exactly once;
  - the archived identity is absent;
  - no tmux diagnostic is present; and
  - exactly one coherent same-width outer border pair remains.
