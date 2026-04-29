#!/usr/bin/env bash
# Smoke scenario: tui-mouse-wheel-does-not-send-arrows (fix-mouse-wheel-2)
#
# Pins the contract that mouse-wheel events in the wg TUI MUST scroll the
# OUTER vt100 pane and MUST NOT translate to arrow-key bytes forwarded to
# the embedded child's stdin. Pre-fix, claude code emitted
# 'Scroll wheel is sending arrow keys · use PgUp/PgDn to scroll in claude
# code' because the wg TUI was sending it Up/Down on every wheel notch.
#
# Runs the relevant cargo tests; both assertions live there:
#   * mouse_wheel_in_vendor_pty_mode_scrolls_outer_not_inner
#       - is_scrolled_back() == true after wheel (outer scrolled)
#       - child_input_bytes_written() unchanged (no PTY stdin write)
#   * mouse_wheel_in_chat_observer_mode_scrolls_vt100_pane (regression
#       guard for the original fix-mouse-wheel observer-mode path)
#
# No live LLM endpoint required — pure unit tests over the event router.
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

echo "=== fix-mouse-wheel-2: wheel must NOT send arrow keys to inner PTY ==="
echo "--- vendor-PTY mode test ---"
cargo test --bin wg --quiet \
    mouse_wheel_in_vendor_pty_mode_scrolls_outer_not_inner 2>&1
echo "--- observer mode regression guard ---"
cargo test --bin wg --quiet \
    mouse_wheel_in_chat_observer_mode_scrolls_vt100_pane 2>&1

echo "=== All fix-mouse-wheel-2 tests passed ==="
