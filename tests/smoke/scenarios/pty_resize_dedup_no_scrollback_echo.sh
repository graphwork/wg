#!/usr/bin/env bash
# Scenario: pty_resize_dedup_no_scrollback_echo
#
# Guards the fix for the TUI PTY scrollback duplication bug after SIGWINCH
# reflow. Originally landed as a navigation-time dedup heuristic
# (fix-tui-pty); replaced with true re-feed reflow in fix-scrollback-reflow
# (per diagnose-scrollback-corruption).
#
# Root cause: vt100 0.16's `Screen::set_size` does NOT reflow the scrollback
# `VecDeque<Row>` — only the visible row Vec is resized
# (vt100/src/grid.rs:66-100). Width changes leave stale wrap state in
# scrollback rows, so the user sees old wrapped rows duplicated against the
# new width whenever the child reprints in response to SIGWINCH.
#
# Fix: `PtyPane::resize` snapshots the existing scrollback + visible content
# into logical lines (joining rows with `wrapped()=true`), builds a fresh
# `vt100::Parser` at the new dimensions, and re-feeds the lines so vt100
# rewraps them at parse time. The dedup machinery
# (`scrollback_hidden` / `pending_dedup`) is gone — the bug is fixed at the
# data layer instead of papered over at the navigation layer.
#
# This scenario re-runs the unit tests that pin the contract:
#
#   tui::pty_pane::tests::naive_set_size_then_child_reprint_creates_scrollback_duplicates
#     — pre-condition: confirms vt100's plain set_size + reprint produces
#       duplicates, locking in the bug shape for regression detection.
#
#   tui::pty_pane::tests::refeed_reflow_eliminates_scrollback_duplicates
#     — primary fix assertion: re-feed reflow produces no duplicates.
#
#   tui::pty_pane::tests::refeed_reflow_rewraps_at_narrower_width
#   tui::pty_pane::tests::refeed_reflow_unwraps_at_wider_width
#     — width-change semantics: lines re-wrap / un-wrap with each appearing
#       exactly once.
#
#   tui::pty_pane::tests::refeed_reflow_handles_burst_of_resizes_without_compounding_duplicates
#     — typing-burst SIGWINCH stress: multiple resizes do not compound dups.
#
#   tui::pty_pane::tests::pty_pane_resize_does_not_create_scrollback_duplicates
#     — end-to-end: drives a real spawned PtyPane through a resize burst and
#       asserts the integrated path (snapshot + swap parser + master.resize)
#       leaves scrollback duplicate-free.
#
# Exit 77 (SKIP) is NOT used — these tests run against the in-process vt100
# parser and a /bin/sh PTY child; no external endpoint or network is needed.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

TESTS=(
    "tui::pty_pane::tests::naive_set_size_then_child_reprint_creates_scrollback_duplicates"
    "tui::pty_pane::tests::refeed_reflow_eliminates_scrollback_duplicates"
    "tui::pty_pane::tests::refeed_reflow_rewraps_at_narrower_width"
    "tui::pty_pane::tests::refeed_reflow_unwraps_at_wider_width"
    "tui::pty_pane::tests::refeed_reflow_handles_burst_of_resizes_without_compounding_duplicates"
    "tui::pty_pane::tests::refeed_reflow_then_child_repaint_keeps_scrollback_almost_clean"
    "tui::pty_pane::tests::pty_pane_resize_does_not_create_scrollback_duplicates"
)

echo "running pty_pane re-feed reflow unit tests..."
# `cargo test --bin wg` exercises the main binary crate where
# `src/tui/pty_pane.rs` (and its #[cfg(test)] block) lives.
# Note: cargo test accepts exactly one filter arg; run the tests separately.
for t in "${TESTS[@]}"; do
    echo "  $t"
    if ! cargo test --bin wg "$t" -- --exact 2>&1; then
        echo "FAIL: $t"
        exit 1
    fi
done

echo ""
echo "PASS: pty_resize_dedup — re-feed reflow keeps scrollback duplicate-free across SIGWINCH"
exit 0
