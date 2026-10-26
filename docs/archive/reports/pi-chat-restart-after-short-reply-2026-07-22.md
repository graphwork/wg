# Pi chat restart after a short reply — 2026-07-22

**Task:** `diagnose-pi-chat-restart`

**Production chat:** `.chat-8` (read-only throughout this investigation)

**Follow-up:** `fix-durable-pi-chat-exit`

## Executive diagnosis

A real interactive Pi process ended and a later explicit TUI recovery created a new Pi process in a newly-created tmux session with the same canonical identity. This was **not** a daemon restart, a WG TUI restart, a tmux-server restart, or a mere repaint:

- observed Pi pane PID `2567909` started at `2026-07-22T08:26:36.689571Z`;
- its systemd tmux scope ended at `08:27:05.869722Z` after 4.276 s CPU;
- replacement Pi PID `2571028` started at `08:28:45.988526Z`;
- daemon PID `1442392`, TUI PID `1442449`, and tmux server PID `3050662` all remained the same processes;
- the current tmux session's creation epoch is `1784708925` (`08:28:45Z`), so this is a new session with the reused canonical name, not an attach to the previous inner child.

The replacement was TUI-owned, not daemon-supervisor-owned. WG deliberately suppresses automatic respawn after a pane death and displays a death panel; an explicit `R` or explicit navigation back to the still-live chat clears the volatile death state and calls the same exact-identity spawn path. The exact gesture is not audited, but the decision class is unambiguous: explicit TUI resurrection created the missing canonical tmux session. The daemon's `.chat-8` supervisor had already returned at `08:25:28Z` under its idle/no-respawn rule.

The **original inner-child exit code/signal is no longer recoverable**. WG stored only the outer `tmux attach` child's status in the TUI's in-memory `chat_agent_death` map, then deleted that entry on recovery. It never persisted the inner Pi PID/status/signal/stderr or the recovery decision. The user journal proves scope start/end, but a systemd `.scope` `Result=success` describes scope collection and must not be misreported as Pi exit code 0. This observability defect is why a more specific root cause—uncaught Pi exception versus clean quit versus external signal—cannot honestly be selected after the fact.

The evidence falsifies the proposed semantic triggers:

- the one-token text `A` follows the ordinary non-empty submit path; Pi has no length-dependent exit branch;
- the adjacent `thinking_level_change` occurred **after PID 2567909 started**, persisted `xhigh`, and the setter has no restart/exit behavior;
- the seventh and last completed compaction occurred on `2026-07-21T14:16:21.544Z`, about 18 hours before the incident; no compaction entry is adjacent;
- no model change is adjacent (the last persisted model changes were on July 12);
- update checks only add UI notifications and do not invoke `pi update`;
- Pi's raw stdout output guard is disabled in interactive mode;
- WG's PTY growth guard discards parser scrollback and warns; it does not kill the child;
- no OOM, coredump, kernel kill, or user-journal signal record was found; disk admission was warning-only, not refusal.

Therefore the defensible diagnosis is: **an inner Pi lifecycle exit occurred; WG recovered the exact persisted Pi session, but WG discarded the only exit-status observation, so the initiating exception/signal cannot be reconstructed.** Do not attribute the exit to `A`, thinking, compaction, update notification, disk, or OOM without new evidence.

## Authoritative identity and preservation

At the evidence snapshot:

| Field | Authoritative value |
|---|---|
| graph directory | `/home/bot/wg/.wg` |
| graph task | `.chat-8`, `InProgress`, tag `chat-loop` |
| task command identity | `command_argv=["pi"]`, `working_dir=/home/bot/wg`, preset `pi` |
| session UUID | `019f5598-9c36-7e72-bd41-75d7ba727b14` |
| aliases | `chat-8`, `coordinator-8`, `8` |
| canonical chat directory | `.wg/chat/019f5598-9c36-7e72-bd41-75d7ba727b14` |
| Pi session ID | `chat-8` |
| Pi session directory | canonical chat directory + `/pi-sessions` |
| transcript | `2026-07-12T09-11-33-232Z_chat-8.jsonl` |
| transcript header | session version 3, id `chat-8`, cwd `/home/bot/wg` |
| tmux name | `wg-chat-wg-0996c58dbb8632b1-chat-8` |
| write-back identity | `WG_DIR=/home/bot/wg/.wg`, `WG_CHAT_ID=.chat-8`, `WG_CHAT_REF=chat-8`, `WG_EXECUTOR_TYPE=pi` |
| persisted model | `openai-codex/gpt-5.6-sol` in Pi JSONL |
| persisted reasoning | `xhigh` from event `cd0ca6ec` |

The current argv is:

```text
/home/bot/.nvm/versions/node/v25.4.0/bin/pi
  --session-id chat-8
  --session-dir /home/bot/wg/.wg/chat/019f5598-9c36-7e72-bd41-75d7ba727b14/pi-sessions
```

No provider/model flags are present because the WG chat task itself has no model override. This is safe in this incident: Pi reopened the named native session and recovered its persisted model and reasoning. It is also why future recovery evidence must record both WG route metadata and Pi's restored native session state rather than infer either from the live PID.

The transcript was not lost or rewritten by WG. At incident-time inspection it had 2,257 entries and approximately 13.9 MB; it has continued to append since. The incident selection is a single user entry:

```text
2240  message id=da0dab1a  2026-07-22T08:25:54.687Z
      user: "A (response to last query)"
2241  thinking_level_change id=cd0ca6ec parent=da0dab1a
      2026-07-22T08:26:43.342Z  xhigh
2242  message id=16a6c38e parent=cd0ca6ec
      2026-07-22T08:26:44.645Z
      user: "why is pi dying? i said \"A\""
```

There is one `da0dab1a`, not a replayed duplicate. Recovery did not inject a message or replay the turn; it reopened Pi idle on the existing branch. Subsequent messages and every earlier preference response remain present.

## Timeline (UTC)

| Time | Evidence and ownership |
|---|---|
| 2026-07-21 14:16:21.544 | Seventh completed compaction appended. This is the final `compaction` entry before the incident. |
| 2026-07-21 13:57:59 | WG TUI PID `1442449` started and remained alive through the incident. |
| 2026-07-21 13:57:56 | WG daemon PID `1442392` started and remained alive. |
| 2026-07-22 08:25:12.406 | Last daemon log line saying `.chat-8`'s persistent TUI tmux pane was live. |
| 08:25:27.466 | With that tmux session absent, daemon tried `wg ... spawn-task .chat-8` using executor `pi` and model `openai-codex:gpt-5.6-sol`. This was the pipe/RPC adapter path, not the visible vendor pane. |
| 08:25:28.631 | Daemon child exited; supervisor classified the chat idle and returned: `exiting supervisor (no respawn)`. `.handler.pid` is now stale adapter metadata and does not describe the visible Pi. |
| 08:25:54.687 | The one-word preference reply was appended once to the authoritative Pi session. The exact pane PID handling this first apparent failure was not durably recorded. |
| 08:26:36.689 | User journal: tmux server launched observed pane PID `2567909`. This is an explicit visible-Pi recovery/start. |
| 08:26:43.342 | Pi appended `thinking_level_change=xhigh`, seven seconds after PID 2567909 began. It therefore did not cause the prior apparent death. |
| 08:26:44.645 | User reported the death in the same authoritative Pi session. |
| 08:27:00.625–08:27:01.163 | PID 2567909 began the diagnostic turn and persisted six ordinary read-only tool results (status, session registry, chat list, process list, disk doctor, recent output files). |
| 08:27:05.869 | User journal: the exact `2567909` tmux scope ended after 4.276 s CPU. No final assistant message was appended. No kernel/user-journal OOM/signal record exists. |
| 08:28:45.988 | User journal: tmux server launched replacement pane `2571028`. Current tmux `session_created` is the same second, proving a new underlying session rather than a mere outer-client reattach. |
| 08:29:16.102 onward | User continued; Pi appended messages and tool results to the same JSONL. |

The tmux server did not restart: PID `3050662` has run since June 23. The TUI did not restart: PID `1442449` has run since July 21. Only the underlying chat session/Pi child was replaced.

## Lifecycle audit

### TUI and tmux ownership

`PtyPane::spawn_via_tmux` creates the canonical tmux session only when it is missing and otherwise attaches to it (`src/tui/pty_pane.rs:397-477`). The `PtyPane` child is **`tmux attach`**, while the real Pi process lives inside tmux (`src/tui/pty_pane.rs:103-113`). Dropping the TUI pane kills only the attach client and intentionally does not kill the tmux session (`src/tui/pty_pane.rs:977-993`).

At startup, if the canonical tmux session already exists, the loader writes the TUI sentinel and requests a reattach with a fail-closed dummy command that must never be run (`src/tui/viz_viewer/state.rs:8479-8520`). If the session is missing, the loader builds the exact Pi `--session-id`/`--session-dir` invocation after canonical storage registration (`src/tui/viz_viewer/state.rs:18252-18312`). Canonical WG child identity comes from `chat_pty_env` (`src/tui/viz_viewer/state.rs:4296-4311`).

The daemon recognizes the persistent vendor tmux session as owner and defers (`src/commands/service/coordinator_agent.rs:916-932`). It separately defers to a live TUI sentinel (`:934-953`). It refuses daemon respawn for archived/Done/Abandoned chats (`:884-906`).

### What happens on death

The render path polls the outer PTY child. When it is no longer alive, it calls `try_exit_status_desc`, puts status/executor/command in `chat_agent_death`, removes the pane, and clears the TUI sentinel (`src/tui/viz_viewer/render.rs:3856-3892`). It intentionally does **not** auto-spawn while death information exists (`:3898-3905`).

The status being polled is the outer `tmux attach` client (`src/tui/pty_pane.rs:937-955`), not the inner Pi process. It is formatted only as `exit code N`; signal/core detail is not represented. The death map is process memory only (`src/tui/viz_viewer/state.rs:4341-4352`, `:7772`).

Recovery keys are explicit: `R` removes the map entry and invokes `maybe_auto_enable_chat_pty`; `E` removes it and opens the launcher; `X` removes it and falls back to history (`src/tui/viz_viewer/event.rs:2605-2636`). Selecting a live chat can also clear a prior death entry before switching (`src/tui/viz_viewer/state.rs:17620-17637`). With the canonical tmux session gone, `spawn_via_tmux` creates a new one under the same name and Pi loads the same named transcript.

This explains both continuity and missing forensics: the recovery identity is stable, but the exit observation is volatile and describes the wrong process boundary.

### Storage and terminal-state safety

`prepare_pi_chat_session` completes UUID registration/migration before discovering or creating the `pi-sessions` directory and picks an existing `_chat-8.jsonl` transcript (`src/chat_sessions.rs:607-649`). `chat_is_live` requires a graph task with matching chat ID that is nonterminal and non-archived (`src/tui/viz_viewer/state.rs:17572-17583`). Terminal/archived chats are routed to canonical Detail rather than an interactive composer (`:17585-17620`).

Runtime files do not authorize recovery:

- `.tui-driven` contains only TUI PID and timestamp;
- `.handler.pid` describes the stale daemon adapter PID/kind, not the vendor Pi;
- `coordinator-state-8.json` only retains an old `executor_override=pi` and no model;
- tmux holds live process state but destroys it with the session;
- the Pi JSONL holds conversation/model/reasoning, not OS exit reason or WG recovery decision.

### Pi behavior at the suspected triggers

The exact installed package is `@earendil-works/pi-coding-agent 0.80.10`. Relevant installed files were hashed during the investigation:

```text
601952fe...14932f2  dist/modes/interactive/interactive-mode.js
ba869d5d...e713fc8  dist/core/agent-session.js
```

1. **One-token input.** Interactive submit trims text and rejects only empty text. An ordinary `A` is queued through the same path as any other prompt (`interactive-mode.js:2076-2260`). The run loop catches `session.prompt()` errors and displays them rather than exiting (`:580-657`).
2. **Thinking.** `cycleThinkingLevel` calls `setThinkingLevel`, which clamps, appends one setting event, updates settings, and emits callbacks; it does not restart (`agent-session.js:1281-1314`).
3. **Model.** `setModel`/cycle append model and thinking changes in-process (`agent-session.js:1200-1270`). No `model_change` occurred near this incident.
4. **Compaction.** Compaction emits start/end, appends a summary, rebuilds in-memory messages, reports failure, and reconnects to the agent in `finally` (`agent-session.js:1373-1484`). The UI rebuilds the chat after a successful compaction and remains interactive (`interactive-mode.js:2441-2482`).
5. **Update notices.** Startup asynchronously checks and renders “Run pi update” / package-update instructions (`interactive-mode.js:580-613`, `:3158-3190`). It does not execute an update. The installed executable timestamp remained July 18 throughout the incident.
6. **Output guard.** `main.js:421-425` takes over raw stdout only when app mode is not interactive. WG's separate PTY rate guard merely sets a warning flag and clears parser scrollback above 512 KiB/s (`src/tui/pty_pane.rs:63-72`, `:283-305`).
7. **Pi exits.** Interactive Pi can exit cleanly on `/quit`, empty-editor Ctrl+D, double Ctrl+C, SIGTERM/SIGHUP graceful shutdown, exit 129 on dead-terminal emergency, or exit 1 on an uncaught exception (`interactive-mode.js:2790-2944`). The missing durable inner status prevents choosing among these branches.

## Isolated reproduction and falsification matrix

No destructive action was run against production. The probe copied the full long JSONL into `/tmp`, removed all `WG_*` identity variables, used a separate tmux socket, and never submitted a provider request.

The successful long-session probe loaded all seven compactions, then:

- performed ten `A` edit/delete cycles without submission;
- sent Pi's `Shift+Tab` thinking control;
- observed one new copied `thinking_level_change=max` entry;
- observed the exact same Pi PID `2612834` before/after the short input and thinking change.

A corrected observer probe held client stdin open, attached a first tmux client, attached a second with `-d`, and detached them through tmux. Pi PID `2629033` and `pane_dead=0` remained stable throughout. An earlier harness attempt allowed the synthetic attach client's stdin to reach EOF, which correctly made Pi perform its normal Ctrl+D shutdown; that harness artifact is excluded from the result.

A separate clean crash probe enabled tmux `remain-on-exit` only in the isolated socket, SIGKILLed Pi PID `2644988`, and captured `pane_dead=1`, empty exit status, and `pane_dead_signal=9`. Production WG does not enable `remain-on-exit`, so equivalent inner-child death destroys the one-pane session and loses those tmux fields.

| Candidate | Runtime result | Source result | Verdict |
|---|---|---|---|
| repeated one-character editing | 10 isolated cycles, PID stable | no length-specific exit path | falsified as a local/TUI trigger; submitted provider turn not replayed credential-free |
| thinking change | copied JSONL appended setting; PID stable | in-process setter only | falsified |
| model change | no adjacent production event | in-process setter only | falsified for this incident |
| prior compactions / reload | copied 7-compaction session loaded and remained live | successful compaction rebuilds UI; errors are surfaced | prior compaction falsified; a novel uncaught bug during an unrecorded in-flight compaction cannot be ruled out without exit stderr |
| Pi update notice | appeared in isolated session without exit | display-only | falsified |
| TUI detach/reattach | proper tmux clients left exact PID stable | attach-only when session exists | falsified |
| concurrent observer | second `attach -d` left exact PID stable | only outer attach is replaced | falsified |
| child crash/signal | isolated SIGKILL removed inner session | production scope ended, but reason not persisted | remains possible |
| WG TUI restart | production TUI PID unchanged | no evidence | falsified |
| daemon restart/respawn | daemon PID unchanged; `.chat-8` supervisor had returned | daemon defers to tmux/TUI owner | falsified for replacement |
| OOM/disk | no OOM/coredump; approximately 62 GiB free at incident, warning only | no hard refusal | falsified on available evidence |

The only non-credential-free gap is an actual sequence of paid one-token model turns across a fresh compaction boundary. That gap does not justify mutating the production session. Source behavior, multiple production `A` turns that completed normally, and the copied-session controls make input length an implausible cause; a future fake-provider smoke belongs in the follow-up.

## Required fix (not implemented here)

A narrow safe patch is **not** obvious because the process boundary is wrong: the TUI owns `tmux attach`, not Pi, and recovery/terminal-state authority spans tmux, graph metadata, UUID storage, and Pi's native session. Patching only the death-panel text would risk reporting the attach client's status as Pi's and would not meet the no-duplicate-turn/terminal-chat requirements.

`fix-durable-pi-chat-exit` specifies the proper build:

1. wrap or hook the inner vendor process and append a redacted start/exit ledger under the canonical UUID chat directory;
2. record inner PID, code/signal, exact identity/route/session dir/tmux name and stderr evidence;
3. durably record reattach/restart/refusal decisions and a bounded attempt count;
4. expose one reason consistently in `wg chat show`, daemon log, and TUI;
5. make recovery send zero stdin and never replay a turn;
6. continue to require authoritative nonterminal/non-archived graph state;
7. validate with a fake Pi and real tmux/PTY human flow.

To avoid overlapping edits, `implement-approved-symbolic-tui` now depends on this follow-up.

## Evidence commands

Representative read-only commands:

```bash
# Stable owners and current inner child
ps -p 1442392,1442449,3050662,2571028 -o pid,ppid,lstart,etime,stat,cmd

tmux list-panes -t wg-chat-wg-0996c58dbb8632b1-chat-8 \
  -F 'session_created=#{session_created} pane_pid=#{pane_pid} pane_start_command=#{pane_start_command}'

# Historical exact PID launches/scope end
journalctl --user --since '2026-07-22 08:26:30 UTC' \
  --until '2026-07-22 08:29:00 UTC' -o short-iso-precise

# No OOM evidence in accessible journals
journalctl --user --since '2026-07-22 08:20 UTC' --until '2026-07-22 08:31 UTC'
journalctl -k --since '2026-07-22 08:20 UTC' --until '2026-07-22 08:31 UTC'

# Disk admission
wg disk doctor --json
df -h / /tmp

# Session event/count audit (Python JSON parsing; no writes)
# type counts: message=2245 at the first snapshot, thinking=7,
# compaction=7, model_change=6, custom=6, session=1.
```

No kernel OOM record, user-journal kill record, process-accounting record, coredump tool/data, Pi stderr capture, tmux pane-death hook record, or durable WG exit ledger was available. That absence is reported as an evidence limit, not converted into a guessed exit code.
