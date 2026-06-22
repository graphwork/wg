# TUI Keymap Verification - 2026-06-22

Task: `bug-tui-keymap-verify`

## Result

Verified the TUI keymap/input-routing fix against
`docs/bugs/tui-keymap-routing.md`. The implementation is policy-based: focused
PTY/input routing is handled before global graph/window hotkeys, so `+` is not
treated as a one-off exception.

## Policy Match

- P1, focused input first: `src/tui/viz_viewer/event.rs` routes keys through
  the active PTY/focused-input branch before normal global hotkey handling.
  Bare printable input is guarded by `is_bare_printable`.
- P2, globals gated on no-input-focus: launcher/window hotkeys such as `+`,
  `n`, and `w` are only reachable after the focused PTY branch declines the
  event. Tests keep command-mode behavior separate from PTY-focused behavior.
- P3, reserved passthrough: `Ctrl+T` is not a host command in PTY focus; it is
  forwarded to the child process for executor thinking-toggle conventions.
- P4, command-mode rebind: `Ctrl+O` is the host escape/toggle between PTY focus
  and command mode. The footer/hints in `render.rs` advertise `Ctrl+O`.

## Code And Test Evidence

- `src/tui/viz_viewer/event.rs`: `is_bare_printable`,
  `is_command_mode_toggle`, the PTY-focused routing branch, and comments tying
  the branch to the keymap policy.
- `src/tui/viz_viewer/render.rs`: `[PTY] Ctrl+O: command mode` and
  `[CMD] Ctrl+O: back to chat` hints.
- Unit coverage in `src/tui/viz_viewer/event.rs`:
  `ctrl_o_toggles_pty_modal_state`,
  `pty_mode_ctrl_t_forwards_to_child_not_command_mode`,
  `pty_mode_plus_forwards_to_child_not_launcher`,
  `pty_mode_all_bare_printables_forward_to_child`, and
  `test_command_mode_plus_opens_launcher`.
- Smoke coverage:
  `tests/smoke/scenarios/tui_keymap_printables_and_ctrl_t.sh`, owned by
  `bug-tui-keymap-verify` in `tests/smoke/manifest.toml`, drives a real
  `wg tui` under tmux and asserts raw bytes `+`, `a`, and `Ctrl+T` reach the
  focused child, then asserts `Ctrl+O` toggles command mode.

## Validation Commands

- `cargo fmt --check`: PASS.
- `cargo clippy`: PASS, with existing warnings only.
- `cargo build`: PASS, with existing warnings only.
- `CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/wg-agent-5539-cargo-target cargo test -j1 -- --test-threads=1`:
  partial PASS before environment failure. It passed `src/lib.rs`
  (2407 tests), `src/main.rs` (3578 passed, 1 ignored), all keymap unit tests,
  and many integration binaries. It then failed when `/tmp` filled during
  `tests/integration_evolver_pipeline.rs`; each failing case reported
  `No space left on device`.
- A focused rerun of `integration_evolver_pipeline` after clearing this task's
  alternate target also failed during dependency build for the same environment
  reason: `boring-sys2` could not write `libssl.a` because the filesystem was
  full. Large build targets existed in other active WG worktrees and were left
  untouched.
- `bash tests/smoke/scenarios/tui_keymap_printables_and_ctrl_t.sh`: PASS.

## Manual Verify Checklist

1. Start `wg tui` and focus/open a chat PTY. Type `+`. Expected: `+` is
   inserted into the child prompt/input, and the add-chat launcher/modal does
   not open.
2. Press `Ctrl+T` while the chat PTY is focused. Expected: the embedded
   executor receives the thinking-toggle key (`0x14`) / toggles thinking
   behavior; WG remains in `[PTY]` focus and command mode does not open.
3. Press `Ctrl+O`. Expected: WG switches to command mode (`[CMD]` / graph
   focus). Press `Ctrl+O` again to return to `[PTY]`.
