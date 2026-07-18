#!/usr/bin/env bash
# Scenario: tui_chat_service_health_restart
#
# Drives the real TUI on its continuously-refreshing Chat tab, stops and
# restarts the dispatcher daemon, and verifies the visible HUD/vitals recover
# from DOWN without changing tabs or restarting the TUI. A custom `cat` chat
# keeps the flow credential-free and lets us verify chat input remains live.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
# The scenario intentionally exercises human service controls. Smoke gates may
# inherit a worker identity from `wg done`; do not let that test harness context
# turn the real start/stop flow into an authorization failure.
unset WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER
if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive the real TUI"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-chat-health-$$"
cleanup_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook cleanup_session

cd "$scratch"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg init --no-agency >init.log 2>&1 \
    || loud_fail "wg init failed: $(tail -10 init.log)"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg chat new --name health-probe --command cat >chat.log 2>&1 \
    || loud_fail "credential-free cat chat creation failed: $(cat chat.log)"

start_wg_daemon "$scratch" --max-agents 1
wg_dir="$WG_SMOKE_DAEMON_DIR"

capture() {
    tmux capture-pane -p -t "$session" 2>/dev/null || true
}

wait_for_screen() {
    local pattern="$1"
    local attempts="${2:-80}"
    local screen
    for _ in $(seq 1 "$attempts"); do
        screen=$(capture)
        if printf '%s\n' "$screen" | grep -q "$pattern"; then
            printf '%s\n' "$screen"
            return 0
        fi
        sleep 0.1
    done
    return 1
}

tmux new-session -d -s "$session" -x 150 -y 45 \
    "env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER wg tui --no-mouse --show-keys"

initial=$(wait_for_screen 'coord ●' 120) \
    || loud_fail "TUI did not show the live daemon on startup: $(capture | tail -15)"
printf '%s\n' "$initial" | grep -q '\[PTY\]' \
    || loud_fail "TUI was not left on the active Chat PTY tab: $(printf '%s\n' "$initial" | tail -15)"

# Force the user-visible stale transition first. This proves the later live
# result is a fresh Service snapshot, not the startup value.
if ! wg --dir "$wg_dir" service stop >stop.log 2>&1; then
    loud_fail "service stop failed: $(cat stop.log)"
fi
down=$(wait_for_screen 'coord ○ down' 120) \
    || loud_fail "TUI Chat tab did not show daemon DOWN after stop: $(capture | tail -15)"
printf '%s\n' "$down" | grep -q '● DOWN' \
    || loud_fail "service-health badge did not show DOWN after stop: $(printf '%s\n' "$down" | tail -15)"
printf '%s\n' "$down" | grep -q '\[PTY\]' \
    || loud_fail "TUI changed away from the Chat PTY while daemon stopped"

# Restart externally while the TUI remains untouched on Chat. The old
# Chat-before-Service one-slot ordering can leave the two DOWN strings stuck
# here indefinitely even though this new daemon PID is alive.
if ! wg --dir "$wg_dir" service start --max-agents 1 >restart.log 2>&1; then
    loud_fail "service restart start failed: $(cat restart.log)"
fi
new_pid=$(wait_for_daemon_pid "$wg_dir" 20) \
    || loud_fail "restarted daemon did not become live: $(cat restart.log)"
register_wg_daemon "$new_pid" "$wg_dir"

live=$(wait_for_screen 'coord ●' 120) \
    || loud_fail "TUI Chat tab stayed stale/DOWN after daemon PID $new_pid became live: $(capture | tail -15)"
if printf '%s\n' "$live" | grep -q '● DOWN\|coord ○ down'; then
    loud_fail "TUI mixed a stale DOWN snapshot into the recovered live HUD: $(printf '%s\n' "$live" | tail -15)"
fi
printf '%s\n' "$live" | grep -q '\[PTY\]' \
    || loud_fail "TUI changed away from the Chat PTY during daemon restart"

# Service-first fairness must not cost Chat responsiveness. Type through the
# same real embedded PTY after recovery and require the cat echo promptly.
payload="health-chat-responsive-$$"
tmux send-keys -t "$session" "$payload" Enter
wait_for_screen "$payload" 50 >/dev/null \
    || loud_fail "Chat PTY stopped responding after service-health recovery: $(capture | tail -15)"

echo "DOWN observed on Chat tab, restarted daemon pid=$new_pid"
echo "LIVE observed without tab/TUI restart; chat echoed '$payload'"
echo "PASS: Chat-tab service health recovers fairly after daemon restart and chat remains responsive"
exit 0
