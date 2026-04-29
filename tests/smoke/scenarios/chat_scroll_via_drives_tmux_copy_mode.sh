#!/usr/bin/env bash
# Scenario: chat_scroll_via_drives_tmux_copy_mode
#
# fix-scroll-via: scrolling a tmux-wrapped chat must drive tmux's own
# copy-mode (so tmux re-emits a clean post-scroll view) rather than
# walking the outer vt100 parser's scrollback (which contains tmux
# repaint frames and renders garbled after the first few lines).
#
# Strategy:
#   1. Start a wg-chat-test-* tmux session at the canonical schema with
#      a long-lived inner shell.
#   2. Disable mouse mode (the production code does this at
#      spawn_via_tmux, but here we model it directly so the smoke
#      doesn't depend on cargo build).
#   3. Drive copy-mode + cursor-up via tmux send-keys -X (the same
#      commands wg's PtyPane::scroll_up issues).
#   4. Assert tmux's #{pane_in_mode} is 1 (we're in copy-mode).
#   5. Send -X cancel (the same command wg's send_key/send_text issues
#      to auto-exit copy-mode on user typing).
#   6. Assert #{pane_in_mode} is back to 0.
#
# This is the architectural contract: wg owns 'when to scroll and by
# how much.' Tmux just executes scroll commands wg sends. If a future
# regression decides to autonomously enter copy-mode on wheel events
# (or skips the cancel-on-typing step), this scenario fails.
#
# Skips when tmux is not on PATH.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "TMUX MISSING" "tmux not installed; scroll-via path doesn't apply"
fi

scratch=$(make_scratch)
cd "$scratch"

suffix="$$-$(date +%s%N)"
session="wg-chat-test-scroll-via-${suffix}"
register_kill_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook register_kill_session

# Bring up a long-lived tmux session — same schema (wg-chat-*) the
# production spawn_via_tmux uses.
if ! tmux new-session -d -s "$session" -- sh -c 'while true; do sleep 1; done' 2>tmux.log; then
    loud_fail "couldn't start tmux session $session: $(cat tmux.log)"
fi

if ! tmux has-session -t "$session" 2>/dev/null; then
    loud_fail "fixture session $session didn't survive new-session"
fi

# Mirror the production-side mouse-mode disable. wg owns scroll for
# wg-chat-* sessions; mouse-mode 'on' would re-route wheel events into
# tmux's autonomous copy-mode entry, which is exactly the path we're
# disabling.
tmux set-option -t "$session" mouse off 2>/dev/null || true

# Pre-scroll: must NOT be in any tmux mode.
pre_mode="$(tmux display-message -p -t "$session" '#{pane_in_mode}' 2>/dev/null | tr -d ' \n')"
if [[ "$pre_mode" != "0" ]]; then
    loud_fail "expected #{pane_in_mode}=0 before any scroll, got '$pre_mode' \
(tmux session unexpectedly already in copy/view-mode)"
fi

# Drive copy-mode + cursor-up — exactly what PtyPane::scroll_up does
# when its tmux_session is Some.
if ! tmux copy-mode -t "$session" 2>copy.log; then
    loud_fail "tmux copy-mode failed: $(cat copy.log)"
fi
if ! tmux send-keys -t "$session" -X -N 5 cursor-up 2>send.log; then
    loud_fail "tmux send-keys -X cursor-up failed: $(cat send.log)"
fi

# Brief settle: tmux applies the mode synchronously but a status
# refresh may lag the response. Poll for up to 1s.
in_mode=""
for _ in $(seq 1 10); do
    in_mode="$(tmux display-message -p -t "$session" '#{pane_in_mode}' 2>/dev/null | tr -d ' \n')"
    if [[ "$in_mode" == "1" ]]; then break; fi
    sleep 0.1
done
if [[ "$in_mode" != "1" ]]; then
    loud_fail "expected #{pane_in_mode}=1 after copy-mode + cursor-up, got '$in_mode' \
— tmux did not enter copy-mode, so wg-driven scrolling is not actually \
driving tmux's scrollback"
fi

# Send -X cancel — exactly what PtyPane::exit_tmux_copy_mode does
# when send_key / send_text fires on a typing path.
if ! tmux send-keys -t "$session" -X cancel 2>cancel.log; then
    loud_fail "tmux send-keys -X cancel failed: $(cat cancel.log)"
fi

post_mode=""
for _ in $(seq 1 10); do
    post_mode="$(tmux display-message -p -t "$session" '#{pane_in_mode}' 2>/dev/null | tr -d ' \n')"
    if [[ "$post_mode" == "0" ]]; then break; fi
    sleep 0.1
done
if [[ "$post_mode" != "0" ]]; then
    loud_fail "expected #{pane_in_mode}=0 after -X cancel, got '$post_mode' \
— tmux did not exit copy-mode, so user typing would still hit the \
copy-mode interpreter rather than the inner CLI"
fi

# Re-confirm mouse mode is off — the contract is wg owns scroll, not
# tmux's autonomous mouse handler.
mouse_state="$(tmux show-options -t "$session" mouse 2>/dev/null)"
if [[ "$mouse_state" != *"mouse off"* ]]; then
    loud_fail "expected 'mouse off' on wg-chat-* session, got: $mouse_state"
fi

echo "PASS: tmux scroll-via copy-mode round-trip works on $session"
exit 0
