# TUI `wg add` output painted into the graph canvas

## Symptom and scope

A field capture from binary commit
`1cee472d64555638a3bddf83a878a11e4539254f` showed literal chat-agent tool
output such as `Added task:` and `Use --after` displaced across the graph side
of `wg tui`. The graph file itself was valid. This was a physical-terminal
corruption: ratatui's previous buffer described one screen while an
unaccounted writer had moved the real terminal cursor.

The important distinction is:

1. `wg add` **should** write to the chat agent's nested PTY. Its bytes are parsed
   by `vt100`, then ratatui renders the resulting cells in the Chat panel.
2. No host helper, background thread, or TUI-owned child may inherit the outer
   alternate-screen stdout/stderr. A single diagnostic there can move the
   physical cursor without updating ratatui's differential buffer. The next
   legitimate Chat repaint can then land those `Added task:` cells in the graph
   canvas.

The visible text therefore identifies the next repaint, not necessarily the
writer that first desynchronized the terminal.

## Baseline reproduction at `1cee472d`

The old commit was built in a detached worktree and run only in isolated tmux
sessions. The user's process was not attached, signalled, or modified.

The reproducer created:

- one command chat whose shell issued three real `wg add` commands;
- a 200×50 outer tmux pane running the old `wg tui` under
  `strace -ff -yy -e trace=write,writev`;
- `tmux pipe-pane -O` on the outer pane; and
- a stale project-owned `wg-chat-…-chat-99` session, which made the old TUI's
  orphan sweep take a legitimate diagnostic path while graph snapshots and
  chat output were arriving.

The old outer byte stream contained this unpositioned write at offset 2675:

```text
... ESC[?25l
[wg-tui] sweeping orphan chat tmux session:
wg-chat-wg-old-evidence-NelJJY-chat-99 (no live task)\n
ESC[1;1H ... next ratatui differential draw ...
```

`strace -yy` identified the writer unambiguously. TID `2494128` was an
auxiliary thread in the old TUI process, and fd 2 was the **outer** terminal:

```text
write(2</dev/pts/62>, "[wg-tui] sweeping orphan chat tm"..., 44) = 44
write(2</dev/pts/62>, "wg-chat-wg-old-evidence-NelJJY-c"..., 38) = 38
write(2</dev/pts/62>, " (no live task)\n", 16) = 16
```

The chat owner separately reported:

```text
OWNER_READY pid=2494181 fd1=/dev/pts/61 fd2=/dev/pts/61
```

A second run traced the command itself. The `wg add` process wrote its normal
output to the **nested chat PTY**, never the outer ratatui PTY:

```text
write(1</dev/pts/62>, "Added task: Bind deterministic E"..., 93) = 93
write(1</dev/pts/62>, "  Use --after bind-deterministic"..., 60) = 60
```

(The PTY numbers are allocated per run; identity is established by comparing
the outer pane tty and the child-reported/traced tty in the same run.) The
outer `pipe-pane` saw `Added task` only later as cursor-positioned ratatui cell
updates.

After the untracked stderr write, `tmux capture-pane -p` showed the exact class
of physical corruption reported by the user: truncated borders, status text
inside panel rows, and command-output fragments displaced away from the Chat
surface:

```text
 [PTY]   5 tasks (  done  3 open, 2 active) ...
.chat-1  ...
┌──────────────────────────────────────────────────                           ┐
.chat-0  ... │ 4:Log [events] │ 5:Coord │ 6:Dash │
...
│eased-peer)                                                                  │
...
-99 (no live t3sk) ... last event ...
 0:Chat | PTY | Ctrl+O:command mode  Ctrl+]:scrol  mode
```

This establishes both file descriptors requested by the incident:
`wg add` emitted to the owning nested PTY, while an unrelated TUI diagnostic
emitted to the outer fd and made the subsequent legitimate `wg add` repaint
appear to leak into the graph.

## Existing partial fix and remaining gap

Commit `d886661445178097c3d6d2a10dd7baaf95fac2b8` fixed the known
Close/Archive variant. It centralized runtime tmux `.status()` calls behind a
helper that nulls stdout/stderr and made duplicate session cleanup silent. That
fix is correct and remains covered by `tui_chat_close_lifecycle`.

It was not a complete TUI output audit. Current main still had direct
`eprintln!` sites in:

- chat takeover timeout and inbox/release failures;
- Pi/Dexto preparation and PTY fallback failures;
- orphan tmux-session sweep;
- failed tab-state persistence;
- the PTY growth-rate guard; and
- the no-tmux persistence warning.

Any one of those could recreate physical/differential-buffer divergence even
though every `wg add` subprocess was correctly inside the chat PTY.

## Repair

This repair makes the invariant general:

- runtime TUI code contains no direct `eprintln!` calls;
- actionable failures become ratatui toasts or `ChatStartupState::Error`, so
  they are rendered through the normal frame;
- best-effort cleanup/persistence diagnostics remain terminal-silent;
- the PTY growth guard records its atomic warning state and truncates
  scrollback without printing from its reader thread;
- every runtime host-side tmux `.status()` still goes through
  `status_silently` (`stdout(Stdio::null())`, `stderr(Stdio::null())`);
- host commands in `VizApp` use `.output()`, capturing both streams; and
- interactive chat/vendor commands continue to run only on the slave side of
  `PtyPane`, where output is consumed by the `vt100` parser.

No full clears, blocking waits, or synchronous graph reads were added. Input,
PTY reading, async snapshots, and ratatui rendering keep their existing
nonblocking lanes.

Two source-policy unit tests prevent regressions:

- `tui_runtime_never_writes_process_stderr`; and
- `tui_host_helpers_never_inherit_the_outer_terminal`.

## Subprocess launch audit

| Launch surface | Output boundary | Result |
|---|---|---|
| `VizApp::exec_command` (`state.rs`) | `.output()` captures stdout + stderr on its worker thread | safe |
| Chat PTY startup (`chat_startup.rs`) | `PtyPane::spawn_in` / `spawn_via_tmux`; child stdout + stderr are the slave PTY | safe and intentionally visible in Chat |
| Runtime tmux IPC (`pty_pane.rs`) | `.output()` captures, or `status_silently` nulls both streams | safe |
| Canonical chat-id tmux ownership (`chat_id.rs`) | probes/rename/kill explicitly capture or null both streams | safe |
| Clipboard helpers (`state.rs`) | `.output()` captures both streams | safe |
| Graph/snapshot/bootstrap/auxiliary workers | no subprocess; all results return over bounded channels | safe |
| Dot/static visualization | runs outside the alternate-screen TUI ownership path | not an outer-screen child |

The audit also found and removed direct background-thread stderr writes, which
are equivalent to inherited subprocess output from ratatui's point of view.

## Permanent live regression

`tests/smoke/scenarios/tui_wg_add_output_confinement.sh` is an installed-binary,
credential-free human-flow test. It:

1. opens a real 200×50 `wg tui` inside tmux with `MOSH_CONNECTION` set;
2. runs a command chat in a nested persistent tmux PTY;
3. makes that chat issue four `wg add` operations with normal `Added task:` and
   `Use --after` output;
4. resizes the outer pane down and back while rapid graph snapshots arrive;
5. compares the physical `capture-pane` surface with `wg tui-dump`;
6. proves the child stdout/stderr tty differs from the outer TUI tty;
7. asserts all command output begins after the Chat-panel divider and every
   graph task row appears exactly once;
8. checks any contiguous command-output bytes in `pipe-pane` are preceded by an
   explicit cursor address in the Chat panel;
9. deliberately makes `tui-state.json` unwritable to exercise the formerly
   noisy diagnostic path; and
10. drives post-output Close → Hide to prove input is still live and the final
    graph-only physical frame has no stale command text, duplicate rows, or
    displaced borders.

The manifest owner is `reproduce-and-fix`, so `wg done` runs this scenario as a
hard gate. The existing archive repaint, stateful restart, path-unique
ownership, non-mutating open, and immediate-chat scenarios remain independent
coverage for adjacent lifecycle paths.
