#!/usr/bin/env bash
# Scenario: launcher_focus_new_chat_on_launch (fix-new-chat-4)
#
# Regression lock for the 2026-05-01 user-reported bug: clicking
# Launch on the new-chat dialog flipped focus back to whichever chat
# tab was active BEFORE the dialog opened, instead of focusing the
# freshly-created chat. Per the spec, after Launch the new tab must
# be the active one and the content area must show a "Booting
# <executor>..." placeholder until the PTY emits its first output.
#
# The TUI focus model can't be driven from a smoke script, so this
# scenario runs the unit tests that pin the contract:
#
#   * drain_commands_create_coordinator_dismisses_launcher_and_switches
#       — Existing fix-tui-new lock: when CreateCoordinator IPC
#         returns success, the launcher is dismissed and focus
#         switches to the new cid in the same drain step.
#
#   * drain_commands_create_coordinator_keeps_focus_on_new_chat_when_graph_lags
#       — fix-new-chat-4 lock: even when sync_active_tabs_from_graph
#         can't yet see the freshly-created chat in graph.jsonl
#         (filesystem caching, stat granularity, race with the
#         daemon write), the trailing force_refresh's
#         "active_coordinator_id missing → switch to active_tabs[0]"
#         fallback must NOT fire and flip focus back to the
#         previous chat. The fix eagerly adds the new cid to
#         active_tabs and tightens the auto-switch condition.
#
#   * drain_commands_create_coordinator_failure_resets_creating_flag
#       — Negative control: when the IPC FAILS, the launcher must
#         stay visible AND active_coordinator_id must NOT change.
#         (Same code path; a careless fix could regress the failure
#         case.)
#
#   * test_chat_booting_placeholder_renders_for_unspawned_pty_pane
#       — The booting placeholder must render the new chat's label
#         and the resolved executor name when chat_pty_mode is on
#         but the PTY pane has not yet spawned. It must NOT fall
#         through to the generic "Press 'c' to type" empty state.
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

echo "=== fix-new-chat-4: Launch focuses the new chat tab ==="

echo "--- Focus switches to new cid on IPC success ---"
cargo test --bin wg --quiet \
    drain_commands_create_coordinator_dismisses_launcher_and_switches 2>&1

echo "--- Focus stays on new chat even when graph hasn't caught up ---"
cargo test --bin wg --quiet \
    drain_commands_create_coordinator_keeps_focus_on_new_chat_when_graph_lags 2>&1

echo "--- Failure path leaves focus untouched (negative control) ---"
cargo test --bin wg --quiet \
    drain_commands_create_coordinator_failure_resets_creating_flag 2>&1

echo "--- Booting placeholder renders before PTY emits its first byte ---"
cargo test --bin wg --quiet \
    test_chat_booting_placeholder_renders_for_unspawned_pty_pane 2>&1

echo "=== All fix-new-chat-4 tests passed ==="
