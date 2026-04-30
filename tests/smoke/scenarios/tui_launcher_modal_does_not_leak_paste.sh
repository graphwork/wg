#!/usr/bin/env bash
# Smoke scenario: tui-launcher-modal-does-not-leak-paste (fix-paste-events)
#
# Pins the contract that while the new-chat launcher (or any non-Normal
# `InputMode`) is open, ZERO bytes of an `Event::Paste` may reach the
# underlying chat-pane PTY child's stdin. fix-new-chat-2 closed the
# keystroke path but missed paste events: crossterm delivers bracketed
# paste as a single `Event::Paste(String)` rather than a sequence of
# `Event::Key`s, so the keystroke-path guard never fired. The user
# repro 2026-04-30 was Cmd-V'ing a tailscale URL into the new-chat
# dialog and watching the URL appear in the background chat tab.
#
# This scenario exercises four unit tests in
# `src/tui/viz_viewer/event.rs::chat_tab_navigation_tests`:
#
#   * launcher_open_does_not_leak_paste_to_underlying_pty
#       - byte-level: child_input_bytes_written() unchanged after
#         dispatch_event(Event::Paste) with launcher open
#   * launcher_open_clears_paste_routing_to_launcher_field
#       - routing-level: paste reaches the launcher's Name field even
#         with chat_pty_mode + forwards_stdin + RightPanel focus all true
#   * launcher_paste_reaches_custom_endpoint_text
#       - the user's exact repro: paste a URL into the Endpoint custom
#         input; the URL must land in launcher.endpoint_picker.custom_text
#   * paste_in_normal_mode_still_reaches_chat_pty
#       - negative control: in Normal mode with chat PTY active, paste
#         must STILL forward to the PTY child (guard does not over-block)
#
# This is the THIRD iteration of the new-chat input-leak bug class
# (fix-new-chat → fix-new-chat-2 → fix-paste-events). Each iteration
# covered ONE event class. Together, the unit tests now exercise both
# Event::Key and Event::Paste under the launcher-open input mode.
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

echo "=== fix-paste-events: launcher modal must NOT leak paste to background PTY ==="
echo "--- byte-level: child_input_bytes_written stays at 0 across paste ---"
cargo test --bin wg --quiet \
    launcher_open_does_not_leak_paste_to_underlying_pty 2>&1
echo "--- routing-level: paste reaches launcher Name field ---"
cargo test --bin wg --quiet \
    launcher_open_clears_paste_routing_to_launcher_field 2>&1
echo "--- repro path: paste reaches launcher Endpoint custom_text ---"
cargo test --bin wg --quiet \
    launcher_paste_reaches_custom_endpoint_text 2>&1
echo "--- negative control: Normal-mode paste still forwards to chat PTY ---"
cargo test --bin wg --quiet \
    paste_in_normal_mode_still_reaches_chat_pty 2>&1

echo "=== All fix-paste-events tests passed ==="
