#!/usr/bin/env bash
# Scenario: tui_opencode_scrollback (fix-opencode-tui)
#
# Live human-flow regression for OpenCode chat scrollback in `wg tui`.
#
# The bug: OpenCode launches its OWN full-screen alternate-screen TUI inside
# WG's tmux-wrapped chat pane. WG's scroll controls (`PtyPane::scroll_*`) drive
# tmux copy-mode for tmux-wrapped panes — but an alt-screen child has NO tmux
# scrollback history to walk (the alt screen only ever holds the current
# repaint frame). So on `main`, scrolling an OpenCode pane:
#   (a) shoves the wrapping wg-chat tmux session into copy-mode (a no-op for
#       the user — there is nothing to scan), and
#   (b) forwards NOTHING to OpenCode itself, so its own message history never
#       moves. The user "cannot scan up in history reliably."
#
# The fix (executor-scoped): for OpenCode panes WG forwards OpenCode's own
# scroll keys (PageUp/PageDown/Home/End == messages_page_up/down/first/last)
# straight into the PTY child, and does NOT touch tmux copy-mode. claude /
# codex / nex keep the tmux copy-mode path.
#
# This drives the REAL TUI/tmux path (not `wg opencode-handler` / `wg chat
# create`): it launches `wg tui` in tmux, opens an OpenCode chat via the `+`
# menu, focuses the live OpenCode PTY pane, and scrolls with the same WG
# controls a human uses. It then asserts BOTH halves of the fix, each of
# which flips between main and the fixed build:
#   1. The OpenCode pane's forwarded-input tee (WG_PTY_DUMP) contains the
#      PageUp / Home escape sequences — WG delivered OpenCode's scroll keys.
#   2. The wrapping wg-chat-*-chat-1 tmux session is NOT in copy-mode — WG did
#      NOT fall back to the broken copy-mode path.
#
# Credential-free: OpenCode only needs to launch its TUI; no model/OpenRouter
# call is made (scroll-key forwarding is entirely WG-side). SKIPs without
# tmux or the opencode binary.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg tui scrollback flow"
fi
if ! command -v opencode >/dev/null 2>&1; then
    loud_skip "MISSING OPENCODE" "opencode binary not on PATH; cannot launch a real OpenCode pane"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-oc-scroll-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
    # Tear down any nested wg-chat sessions wg tui created on the same server.
    for s in $(tmux list-sessions -F '#{session_name}' 2>/dev/null | grep '^wg-chat-' || true); do
        tmux kill-session -t "$s" 2>/dev/null || true
    done
}
add_cleanup_hook kill_tmux_session

fake_home="$scratch/home"
fake_global="$scratch/global"
mkdir -p "$fake_home/.config" "$fake_global"

project="$scratch/project"
mkdir -p "$project"
cd "$project"

# Forwarded-input tee prefix. Each PTY child writes the bytes WG forwards to
# its stdin to `<prefix>.<cmd-basename>.<pid>.in.bin` (see PtyPane WG_PTY_DUMP).
# tmux-wrapped chat panes spawn `tmux attach`, so the OpenCode pane's file is
# `<prefix>.tmux.<pid>.in.bin`.
ptydump="$scratch/ptydump"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" WG_GLOBAL_DIR="$fake_global" \
        wg "$@"
}

if ! run_wg init --no-agency -m claude:opus >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -20 init.log)"
fi

# A live base PTY chat so wg tui starts focused on a chat pane and `+` opens
# the launcher. `cat` keeps this credential-free.
if ! run_wg chat new --name base --command cat --json >base-chat.json 2>&1; then
    loud_fail "creating base cat chat failed: $(cat base-chat.json)"
fi

screen_text() {
    tmux capture-pane -t "$session" -p -S -120 2>/dev/null || true
}

wait_screen_contains() {
    local needle="$1"
    local label="$2"
    local text=""
    for _ in $(seq 1 80); do
        text="$(screen_text)"
        if grep -qF "$needle" <<<"$text"; then
            return 0
        fi
        sleep 0.25
    done
    loud_fail "TUI screen never showed ${label} ('$needle'). Last screen:\n$text"
}

tmux new-session -d -s "$session" -x 180 -y 50 \
    "cd '$project' && env HOME='$fake_home' XDG_CONFIG_HOME='$fake_home/.config' \
     WG_GLOBAL_DIR='$fake_global' WG_PTY_DUMP='$ptydump' wg tui"

wait_screen_contains "[PTY]" "focused base chat PTY"
wait_screen_contains ".chat-0" "base chat tab"

# Open the launcher and create an OpenCode chat (claude, codex, opencode, nex).
tmux send-keys -t "$session" "+"
wait_screen_contains "+ Add new" "default launcher Add-new row"
tmux send-keys -t "$session" Down
sleep 0.1
tmux send-keys -t "$session" Down
sleep 0.1
tmux send-keys -t "$session" Enter
wait_screen_contains "Executor:" "Add-new executor row"
wait_screen_contains "opencode" "opencode executor choice"
tmux send-keys -t "$session" Right
sleep 0.1
tmux send-keys -t "$session" Right
sleep 0.1
wait_screen_contains "◉ opencode" "selected opencode executor"

route="opencode:openrouter/stepfun/step-3.7-flash"
tmux send-keys -t "$session" Tab
sleep 0.1
tmux send-keys -t "$session" "$route"
wait_screen_contains "$route" "typed OpenCode/OpenRouter route"
tmux send-keys -t "$session" Enter

# Wait for the OpenCode chat row to be persisted and the nested wg-chat tmux
# session to come up — that session existing proves the OpenCode PTY pane
# actually spawned (not just metadata written).
for _ in $(seq 1 80); do
    if grep -q '"id":"\.chat-1"' .wg/graph.jsonl 2>/dev/null; then
        break
    fi
    sleep 0.25
done

inner=""
for _ in $(seq 1 120); do
    inner="$(tmux list-sessions -F '#{session_name}' 2>/dev/null | grep -E '^wg-chat-.*-chat-1$' | head -1 || true)"
    if [[ -n "$inner" ]]; then
        break
    fi
    sleep 0.25
done
if [[ -z "$inner" ]]; then
    loud_fail "nested wg-chat-*-chat-1 tmux session never appeared; OpenCode pane did not spawn.\nSessions:\n$(tmux list-sessions 2>&1)\nScreen:\n$(screen_text)"
fi

# Wait for the OpenCode pane's forwarded-input tee to exist (PTY pane spawned
# with WG_PTY_DUMP active). It is created at spawn even before any byte is
# forwarded.
oc_tee=""
for _ in $(seq 1 120); do
    # Newest tmux-attach input tee is the OpenCode pane (base `cat` chat also
    # tmux-wraps, but only the OpenCode pane receives our scroll keys, so any
    # tmux tee that ends up containing PageUp is unambiguously OpenCode's).
    if ls "$ptydump".tmux.*.in.bin >/dev/null 2>&1; then
        oc_tee="present"
        break
    fi
    sleep 0.25
done
if [[ -z "$oc_tee" ]]; then
    loud_fail "no tmux-wrapped PTY input tee ('$ptydump'.tmux.*.in.bin) was created; cannot observe forwarded scroll keys"
fi

# Give the OpenCode TUI a beat to finish its initial paint so the pane is the
# focused, stdin-forwarding chat pane before we scroll.
sleep 1.5

# Drive the SAME scroll controls a human uses. Two routes, both must reach
# OpenCode's own scrollback via WG's scroll_* methods:
#   - direct PageUp/Home while the pane is focused (quick-scroll path)
#   - Ctrl+] scroll-mode, then PageUp/Home (scroll-mode path)
tmux send-keys -t "$session" PageUp
sleep 0.2
tmux send-keys -t "$session" PageUp
sleep 0.2
tmux send-keys -t "$session" Home
sleep 0.2
# Enter WG scroll mode and scroll again.
tmux send-keys -t "$session" C-]
sleep 0.2
tmux send-keys -t "$session" PageUp
sleep 0.2
tmux send-keys -t "$session" Home
sleep 0.5

# ── Assertion 1: WG forwarded OpenCode's own scroll keys to the PTY child ──
# PageUp encodes as ESC[5~, Home as ESC[H. On main these never reach OpenCode
# (scroll drove tmux copy-mode instead), so this grep fails on main.
# PageUp == ESC[5~ ; Home (scroll_to_top → OpenCode messages_first) == ESC[H.
# Built with printf and matched with grep -F so bracket chars stay literal.
pageup_seq="$(printf '\x1b[5~')"
home_seq="$(printf '\x1b[H')"
found_pageup=""
found_home=""
for f in "$ptydump".tmux.*.in.bin; do
    [[ -e "$f" ]] || continue
    if grep -qaF "$pageup_seq" "$f"; then
        found_pageup="$f"
    fi
    if grep -qaF "$home_seq" "$f"; then
        found_home="$f"
    fi
done
if [[ -z "$found_pageup" ]]; then
    loud_fail "OpenCode pane never received a PageUp (ESC[5~) keystroke — WG did not forward OpenCode's own scroll key. Tee files:\n$(ls -l "$ptydump".tmux.*.in.bin 2>&1)\nHexdump (first tee):\n$(od -c "$ptydump".tmux.*.in.bin 2>/dev/null | head -40)"
fi
if [[ -z "$found_home" ]]; then
    loud_fail "OpenCode pane never received a Home (ESC[H) keystroke — scroll_to_top did not forward OpenCode's messages_first key."
fi

# ── Assertion 2: WG did NOT drive the wrapping tmux session into copy-mode ──
# On main, scrolling an OpenCode pane shoves the wg-chat session into copy-mode
# (pane_in_mode=1) while doing nothing useful. The fix keeps it out of
# copy-mode entirely.
in_mode="$(tmux display-message -p -t "$inner" '#{pane_in_mode}' 2>/dev/null | tr -d ' \n')"
if [[ "$in_mode" != "0" ]]; then
    loud_fail "wrapping OpenCode tmux session '$inner' is in copy-mode (pane_in_mode=$in_mode) after scrolling — WG fell back to the broken tmux copy-mode path instead of forwarding OpenCode's own scroll keys."
fi

echo "PASS: wg tui forwards OpenCode's own scroll keys (PageUp/Home) to the PTY and keeps the wrapping tmux session out of copy-mode ($inner)"
exit 0
