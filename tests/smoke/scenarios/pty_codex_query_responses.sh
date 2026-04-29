#!/usr/bin/env bash
# Scenario: pty_codex_query_responses
#
# Guards the fix for the codex chat-tab rendering regression
# (fix-codex-agent). The wg TUI's PTY pane embeds vendor CLIs
# (claude / codex / wg-nex) and acts as their terminal emulator.
# Codex's interactive CLI sends a burst of capability queries on
# startup and BLOCKS waiting for replies — without responses, the
# splash never advances and the chat tab shows nothing useful.
#
# Captured at 24x80 from a fresh `codex` PTY (codex-cli 0.125.0)
# the queries are:
#
#   ESC [ ? 2004 h            (bracketed paste mode set — not a query)
#   ESC [ > 7 u               (push kitty keyboard — not a query)
#   ESC [ ? 1004 h            (focus reporting set — not a query)
#   ESC [ 6 n                 (CPR — cursor position request)        <─ blocks
#   ESC [ ? u                 (kitty keyboard query)                 <─ blocks
#   ESC [ c                   (Primary DA — already answered before)
#   ESC ] 10 ; ? ESC \        (OSC 10 foreground color query)        <─ blocks
#
# A live capture without responses produced exactly 40 bytes (the
# query block) before timeout. With CPR + kitty + OSC 10/11 replies
# in place, codex emits 1000+ bytes of TUI on the next read.
#
# This scenario re-runs the unit tests covering each new query
# response path. Running them as a smoke ensures regressions in
# `compute_query_replies` (or removal of any single handler) are
# caught at `wg done` time — the unit tests run in <0.1s and require
# no live endpoint.
#
# Exit 77 (SKIP) is NOT used — the tests run against the pure
# byte-scanning function directly; no PTY, no live binary, no
# environment dependencies.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

run_test() {
    local filter="$1"
    echo "running $filter ..."
    if ! cargo test --bin wg "$filter" 2>&1; then
        echo "FAIL: $filter"
        exit 1
    fi
}

# Per-query response unit tests — one per RFC the codex CLI cares
# about.
run_test "tui::pty_pane::tests::cpr_query_replies_with_cursor_position"
run_test "tui::pty_pane::tests::cpr_query_uses_real_cursor_position"
run_test "tui::pty_pane::tests::dsr_query_replies_ok"
run_test "tui::pty_pane::tests::kitty_keyboard_query_replies_legacy_mode"
run_test "tui::pty_pane::tests::osc10_foreground_query_replies_with_rgb"
run_test "tui::pty_pane::tests::osc10_foreground_query_with_bel_terminator"
run_test "tui::pty_pane::tests::osc11_background_query_replies_with_rgb"
run_test "tui::pty_pane::tests::osc11_background_query_with_bel_terminator"

# Mixed-stream + multi-query stress.
run_test "tui::pty_pane::tests::cpr_query_in_mixed_stream"
run_test "tui::pty_pane::tests::multiple_queries_yield_concatenated_replies"
run_test "tui::pty_pane::tests::no_query_yields_empty_reply"

# Regression test that uses the actual 40-byte query burst captured
# from codex 0.125.0 at 24x80 — pins the user-reported scenario.
run_test "tui::pty_pane::tests::codex_startup_query_burst_unblocks"

# End-to-end PTY integration: spawns a real bash child inside PtyPane,
# emits the query burst, the bash child reads back the responses wg
# writes through the PTY master, base64-encodes them, and prints a
# marker the test parses. Proves the entire reader-thread →
# respond_to_queries → master-writer → child-stdin loop works, not
# just the pure compute_query_replies fn.
run_test "tui::pty_pane::tests::pty_pane_unblocks_codex_style_query_burst_end_to_end"

echo ""
echo "PASS: pty_codex_query_responses — codex's CPR / kitty / OSC 10/11 startup queries are answered, unblocking the chat-tab TUI"
exit 0
