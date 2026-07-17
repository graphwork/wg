# Recovery audit: `8969864c`

Task: `validate-and-land`

Recovery base: `ca35e2cc`

Reviewed recovery commit: `8969864c6a7ff2c77cb5fe9ef4d14f9a340d7d53`

The recovery commit was applied without committing (`git cherry-pick -n`) and reviewed as WIP. It was not merged wholesale.

## Retained after review

- The shared live-vs-terminal navigation rule in `VizApp::open_coordinator_target` / `open_chat_task_or_detail`, and its use by chooser, tab, prev/next, keyboard, task-row, and pointer paths.
- Terminal/abandoned/archived chats opening canonical Detail without changing the active live-chat identity or requesting a PTY.
- The identity-pinned `ChatCloseContext`, non-mutating Close choice modal, explicit destructive confirmation stage, and canonical `wg chat stop` / `wg chat archive` command effects.
- The labeled `Close…` header control and full-cell pointer hitbox.
- Responsive choice/confirmation rendering and removal of the obsolete disconnected-composer `c`/`:` hint.
- Unit/render coverage for terminal routing, stale Detail rejection, full hitboxes, narrow modal/header rendering, and safe confirmation defaults.

## Rejected or corrected

- **Rejected:** attaching terminal-chat validation to the already latency-heavy `tui_immediate_chat_startup.sh` and assigning ownership to the hidden `.respond-to-restore-immediate-chat` child. That piggyback was brittle and unrelated to the original startup gate. The original scenario was restored byte-for-byte; a dedicated owned installed-binary scenario now covers terminal routing plus every Close lifecycle result.
- **Corrected:** recovery tests did not set the Chat surface while exercising stale header rectangles, causing the recovered suite itself to fail. Tests now model the actual rendered surface.
- **Corrected:** reopening a hidden live chat did not restore tab membership, and hiding the final tab was undone by the first-use bootstrap retry. Reopen now explicitly clears `closed_tabs`/adds the tab; intentional last-tab detach no longer resurrects itself.
- **Corrected:** command-mode chooser access was right-panel-only even though Ctrl+O intentionally focuses the graph. `~`/backtick is now a global Chat command-mode action.
- **Corrected:** Stop succeeded through the daemon but left a TUI-owned PTY running and visible. The success effect now terminates only the identity-matching pane/session and detaches the stopped tab; the graph task remains canonical Open/resumable.
- **Corrected:** custom-command chats inherited the project Pi route in the identity header/modal even while running `command_argv`. Route resolution now pins them to `command` with no unrelated model.
- **Corrected:** destructive confirmation coverage captured the identity but did not prove delayed selection changes could not retarget it. The command effect is now asserted against the originally named chat after an active-selection mutation.
- **Missing from recovery, added:** `.respond-to-*` was parsed as an internal task but fell through to `[∴ evaluating]`. It now produces `[↻ responding]`; the completed parent render and clickable child hit region are tested end to end.

## Landed validation surface

- Focused event/render/state/viz tests in the Rust suite.
- `tests/smoke/scenarios/tui_chat_close_lifecycle.sh`: installed `wg`, real tmux/PTY, abandoned `.chat-36` exact Detail with zero project-session growth, modal identity/state/route, Cancel, Hide, Stop, and Archive.
- Manifest owner: `validate-and-land`.
