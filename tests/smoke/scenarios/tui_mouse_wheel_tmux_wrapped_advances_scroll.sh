#!/usr/bin/env bash
# Smoke scenario: tui-mouse-wheel-tmux-wrapped-advances-scroll (fix-mouse-wheel-3)
#
# Pins the contract that mouse-wheel scroll on a TMUX-WRAPPED chat pane
# (the real chat-tab setup post `implement-tmux-wrapped`) actually
# advances the user-visible scroll position — without writing any
# bytes to the PTY child's stdin.
#
# Why this is separate from `tui_mouse_wheel_does_not_send_arrows`:
# fix-mouse-wheel-2's smoke ran tests against a raw `/bin/sh` PTY
# child (primary screen, real vt100 scrollback). That worked but did
# NOT match the tmux-wrapped reality of real chat panes — tmux uses
# the alt-screen, so the outer vt100 parser has no scrollback to
# advance. fix-mouse-wheel-3 added a tmux IPC dispatch path:
# `tmux send-keys -t <session> -X scroll-up` drives tmux's own copy
# mode, tmux redraws the attached client, our vt100 parser pumps the
# redraw, the user sees scrolled content.
#
# The test asserts BOTH halves of the contract:
#   1. scroll_up changes the rendered content (not a silent no-op)
#   2. scroll_up writes ZERO bytes to the PTY child's stdin
#       (preserves fix-mouse-wheel-2's invariant)
#
# Requires tmux to be installed; SKIPs cleanly if not.
#
# Exit 0  = PASS
# Exit 77 = SKIP (cargo or tmux not available)
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

if ! command -v tmux >/dev/null 2>&1; then
    echo "SKIP: tmux not installed (the fix-mouse-wheel-3 test requires tmux to spawn the wrapped pane)" >&2
    exit 77
fi

cd "$REPO_ROOT"

echo "=== fix-mouse-wheel-3: tmux-wrapped pane scroll must advance render + 0 bytes to child ==="
cargo test --bin wg --quiet \
    tmux_wrapped_scroll_up_advances_render_without_writing_to_child 2>&1

echo "=== Strengthened fix-mouse-wheel-2 vendor-PTY assertion (scrollback() > 0) ==="
cargo test --bin wg --quiet \
    mouse_wheel_in_vendor_pty_mode_scrolls_outer_not_inner 2>&1

echo "=== fix-mouse-wheel-3: all assertions passed ==="
