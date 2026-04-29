#!/usr/bin/env bash
# Smoke scenario: tui-launcher-modal-does-not-leak-keys (fix-new-chat-2)
#
# Pins the contract that while the new-chat launcher (or any non-Normal
# `InputMode`) is open, ZERO keystrokes may reach the underlying chat-pane
# PTY child's stdin. Pre-fix, opening the launcher via the [+] button
# left `focused_panel = RightPanel`, so the `vendor_pty_active` branch in
# `handle_key` ran BEFORE the launcher dispatch and forwarded every typed
# character into the chat-tab child — visible in the user's chat
# conversation as a custom URL appearing where it had no business being.
#
# Same byte-level smoke pattern as fix-mouse-wheel-2:
#   * launcher_open_does_not_leak_keys_to_underlying_pty
#       - child_input_bytes_written() unchanged after typing while modal open
#   * launcher_open_clears_pty_input_routing
#       - keys reach the launcher's Name field even with chat_pty_mode +
#         forwards_stdin + RightPanel focus all true
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

echo "=== fix-new-chat-2: launcher modal must NOT leak keys to background PTY ==="
echo "--- byte-level: child_input_bytes_written stays at 0 ---"
cargo test --bin wg --quiet \
    launcher_open_does_not_leak_keys_to_underlying_pty 2>&1
echo "--- routing-level: keys reach launcher even with PTY mode active ---"
cargo test --bin wg --quiet \
    launcher_open_clears_pty_input_routing 2>&1

echo "=== All fix-new-chat-2 tests passed ==="
