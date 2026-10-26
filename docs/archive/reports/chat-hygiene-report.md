# Chat hygiene report — `chat-hygiene-tidy`

Date: 2026-09-20 (UTC)
Agent: `agent-159`
Worktree: `/home/bot/wg/.wg-worktrees/agent-159`
Chat store: `/home/bot/wg/.wg/chat`

## Outcome

**Report-only. No destructive or lifecycle action taken.** The hard constraint
("must not disturb any live runtime") was satisfied by doing a read-only audit
and leaving every chat entity and directory exactly as found.

- `.chat-4` and `.chat-5` were verified genuinely live and left strictly
  untouched (no stop / resume / reload / fork).
- `.chat-3` was left **as-is** (stopped / Open, preserved transcript).
- Deleted chats `.chat-0/1/2` directories are preserved in place; nothing was
  deleted and no `.archive/` directory exists.

## Before / after `wg chat list`

Both captures are byte-identical (the "after" was taken after all inspection,
including a 45 s live-advance window on the two handlers):

```
ID      STATUS          TASK                      TITLE
0       deleted         .chat-0                   Chat 0
1       deleted         .chat-1                   Chat 1
2       deleted         .chat-2                   Chat 2
3       stopped         .chat-3                   Chat 3
4       supervised      .chat-4                   Chat 4
5       supervised      .chat-5                   Chat: chat3 fork on new binary
```

`.chat-4` and `.chat-5` remain `supervised` before and after, with live handlers
(evidence below).

## Live-handler verification (`.chat-4`, `.chat-5`)

Both have their supervisor tmux session, a live `chat-runtime-wrapper` process,
and advancing session files. Snapshot A → B is a 45 s window
(epoch seconds: A = 1789939344, B = 1789939389).

| chat | tmux session | pane pid | pane cmd | pane dead | wrapper process | session file write | wake-cursor |
|------|--------------|---------:|----------|-----------|-----------------|---------------------|-------------|
| `.chat-4` | `wg-chat-wg-0996c58dbb8632b1-chat-4` | 1125798 | `pi` (exe) | 0 (live) | PID 1125798 alive | `…chat-4.jsonl` mtime 1789938781, 1,130,798 B | 1789939334 → 1789939379 |
| `.chat-5` | `wg-chat-wg-0996c58dbb8632b1-chat-5` | 1973794 | `pi` (exe) | 0 (live) | PID 1973794 alive | `…chat-5.jsonl` mtime 1789939326 → 1789939345, 3,152,406 → 3,157,350 B | 1789939340 → 1789939385 |

`.chat-4`'s transcript last grew ~9 min before snapshot B (idle between turns),
but its per-turn wake cursor advanced and its pane is live; `.chat-5`'s
transcript grew by 4,944 B during the window. Both tmux panes rendered live pi
UI (`π - wg`, `pane_active=1`) with recent transcript text.

## `.chat-3` decision — leave as-is (no archive)

`.chat-3` (`01a0a749-88ae-7c91-8912-3d6e676272b6`) is graph status `open`
(last transition `in-progress → open`, actor `ipc-stop-chat`, reason
`chat_stopped`) and shows as `stopped` in `wg chat list`. Its stored handler PID
1837163 is **dead** (confirmed with `kill -0`), so it is not running.

Choice: **left untouched.** It is the historical parent of the current fork
(`.chat-5` = "Chat: chat3 fork on new binary"), and the fork copied its
transcript from this directory. Archiving would flip the graph task to Done and
tag it `archived`; that is a lifecycle change with no benefit to the live
runtime and a small risk to the parent/fork provenance trail. Per the task's
stated preference ("prefer leaving the transcript intact"), the safe and correct
action is to preserve it in place.

Its transcript is intact and preserved:
`01a0a749-…/pi-sessions/2026-09-15T22-56-49-711Z_chat-3.jsonl` (2,882,791 B).

## Deleted chats (`.chat-0/1/2`) — preserved on disk

All three graph tasks are terminal (`abandoned`, reason `operator_abandoned`).
Their chat directories are preserved **in place** under `.wg/chat/` — nothing
was deleted, and there is **no** `.wg/chat/.archive/` directory. Each still
contains its `pi-sessions/` transcript and runtime logs.

## On-disk footprint of `.wg/chat`

Per-chat directory bytes (`du -sb`):

| chat | task id | uuid dir | bytes | human |
|------|---------|----------|------:|-------|
| `.chat-0` | deleted | `019fe0bd-6661-7961-99da-af2c8af2f9c9` | 6,334,343 | 6.04 MiB |
| `.chat-1` | deleted | `019fec45-6001-7e62-b1e8-6b346fd51235` | 3,976,750 | 3.79 MiB |
| `.chat-2` | deleted | `01a06933-ef40-7040-a71f-6aae43393560` | 6,616,746 | 6.31 MiB |
| `.chat-3` | stopped | `01a0a749-88ae-7c91-8912-3d6e676272b6` | 2,893,128 | 2.76 MiB |
| `.chat-4` | supervised | `01a0c00a-75c8-7783-8904-e60d33e183d5` | 1,137,483 | 1.08 MiB |
| `.chat-5` | supervised | `01a0c02c-1c9a-7f01-a9e7-e0b38fc56200` | 3,166,655 | 3.02 MiB |
| (root files) | — | `sessions.json` (1,508 B) + empty locks/tmp | ~1,508 | ~0.001 MiB |

**Total `.wg/chat/`: 24,126,613 bytes ≈ 23.0 MiB (`du -sh` reports 24M due to
filesystem block rounding).** The three deleted chats account for
16,927,839 B (~16.1 MiB) of that and are the main reclaimable footprint if an
operator later chooses to prune; no pruning was performed here.

## Actions deliberately NOT taken

- Did not run `wg chat stop` / `resume` / `reload` / `fork` on any chat.
- Did not run `wg chat archive` or `wg chat delete` on any chat.
- Did not delete, move, or rewrite any `.wg/chat/**` file.
- The only writes made were the WG audit log entry for this task and this
  report file (in the worktree, not the chat store).

## Reproduce

```bash
wg chat list
tmux ls | grep wg-chat
tmux list-panes -t wg-chat-wg-0996c58dbb8632b1-chat-4 -F '#{pane_pid} #{pane_current_command} #{pane_dead}'
find /home/bot/wg/.wg/chat/<uuid>/pi-sessions -type f -printf '%T@ %s %p\n' | sort | tail -2
du -sb /home/bot/wg/.wg/chat
ls -la /home/bot/wg/.wg/chat/.archive   # → No such file or directory
```
