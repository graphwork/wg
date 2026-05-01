#!/usr/bin/env bash
# Scenario: chat_died_panel_keys_reach_handler (fix-chat-died)
#
# Regression lock for the 2026-05-01 user report: when a chat agent's PTY
# child exits (codex/claude/nex), the TUI shows a 'Chat agent died' panel
# offering R (retry) / E (edit config) / X (dismiss) — but pressing any
# of them did nothing. The user was stuck on the panel with no way out.
#
# Root cause: `vendor_pty_active` in `handle_key` was true even after the
# pane died because `chat_pty_mode` stays set so the render path knows to
# show the death panel. With the pane removed from `task_panes`, every
# keystroke "fell through" to `pane.send_key()` on a missing pane and was
# silently consumed by the unconditional `return` — the death-panel
# recovery branch in `handle_normal_key` was never reached.
#
# Fix: gate `vendor_pty_active` on `!chat_agent_death.contains_key(cid)`
# (both keystroke and paste paths). Also accept SHIFT in the death-panel
# handler so users who type uppercase R/E/X (matching the panel labels)
# don't fall through to other handlers.
#
# The TUI itself can't be driven from a smoke script. Instead, this
# scenario runs the unit tests that pin the contract:
#
#   * r_key_clears_death_info_when_chat_pty_focused
#       — R must clear death info even when the PTY-forwarder thinks
#         it owns input focus (the user-reported regression).
#   * x_key_dismisses_death_panel_when_chat_pty_focused
#       — X must clear death info AND turn off chat_pty_mode so the
#         file-tailing fallback renders.
#   * e_key_opens_launcher_when_chat_pty_focused
#       — E must clear death info and open the launcher.
#   * shift_r_clears_death_info_when_chat_pty_focused
#       — Shift+R (uppercase 'R' as the panel labels suggest) must
#         match too. Pre-fix the handler required `modifiers.is_empty()`
#         which rejected SHIFT.
#   * key_still_reaches_pty_when_no_death_info
#       — Negative control: with no death info, the PTY-forwarder
#         must still work normally. Locks against an opposite-direction
#         regression where the new guard accidentally disables PTY
#         forwarding for live chats.
#
# Exit 0  = PASS
# Exit 77 = SKIP (cargo not available)
# Exit 1  = FAIL

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR" && git rev-parse --show-toplevel 2>/dev/null || echo "")"
if [[ -z "$REPO_ROOT" ]]; then
    echo "SKIP: could not locate git repo root" >&2
    exit 77
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "SKIP: cargo not found" >&2
    exit 77
fi

cd "$REPO_ROOT"

echo "=== fix-chat-died: death-panel R/E/X keys must reach the recovery handler ==="

echo "--- R/E/X reach the death-panel handler from PTY focus ---"
cargo test --bin wg --quiet \
    r_key_clears_death_info_when_chat_pty_focused 2>&1
cargo test --bin wg --quiet \
    x_key_dismisses_death_panel_when_chat_pty_focused 2>&1
cargo test --bin wg --quiet \
    e_key_opens_launcher_when_chat_pty_focused 2>&1

echo "--- Shift+<letter> form also matches the handler ---"
cargo test --bin wg --quiet \
    shift_r_clears_death_info_when_chat_pty_focused 2>&1

echo "--- Negative control: live chat PTY still receives keys ---"
cargo test --bin wg --quiet \
    key_still_reaches_pty_when_no_death_info 2>&1

echo "=== All fix-chat-died tests passed ==="
