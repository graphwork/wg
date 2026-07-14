#!/usr/bin/env bash
# Real human-flow regression for implement-nonblocking-tui.  The storage
# worker is delayed by 1, 3, and 5 seconds while tmux captures the terminal
# directly (the dump socket is intentionally not part of first-frame proof).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive the real TUI"
fi

scratch=$(make_scratch)
cd "$scratch"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER wg init --no-agency >init.log 2>&1 \
    || loud_fail "wg init failed: $(tail -10 init.log)"
env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
    wg add "Slow storage probe" --id slow-storage-probe >add.log 2>&1 \
    || loud_fail "fixture task failed: $(tail -10 add.log)"

sessions=()
cleanup_sessions() {
    local session
    for session in "${sessions[@]}"; do
        tmux kill-session -t "$session" 2>/dev/null || true
    done
}
add_cleanup_hook cleanup_sessions

capture() {
    tmux capture-pane -p -t "$1" 2>/dev/null || true
}

now_ms() {
    date +%s%3N
}

for delay in 1000 3000 5000; do
    session="wgsmoke-async-tui-${delay}-$$"
    sessions+=("$session")
    start=$(now_ms)
    tmux new-session -d -s "$session" -x 120 -y 40 \
        "env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER WG_TUI_TEST_STORAGE_LATENCY_MS=$delay wg tui --no-mouse --show-keys"

    screen=""
    for _ in $(seq 1 100); do
        screen=$(capture "$session")
        if printf '%s\n' "$screen" | grep -q "0 tasks"; then
            break
        fi
        sleep 0.01
    done
    first_ms=$(( $(now_ms) - start ))
    if ! printf '%s\n' "$screen" | grep -q "0 tasks"; then
        loud_fail "${delay}ms storage delay: first shell did not paint within 1s"
    fi
    if (( first_ms >= delay )); then
        loud_fail "${delay}ms storage delay: first frame waited for storage (${first_ms}ms)"
    fi

    key_start=$(now_ms)
    tmux send-keys -t "$session" '?'
    for _ in $(seq 1 50); do
        screen=$(capture "$session")
        if printf '%s\n' "$screen" | grep -q "Navigation"; then
            break
        fi
        sleep 0.01
    done
    key_ms=$(( $(now_ms) - key_start ))
    if ! printf '%s\n' "$screen" | grep -q "Navigation"; then
        loud_fail "${delay}ms storage delay: help key was not accepted during load"
    fi
    if (( key_ms >= 500 )); then
        loud_fail "${delay}ms storage delay: help key took ${key_ms}ms"
    fi

    # Close help and verify the single compact phase slot appears only after
    # the threshold, names its phase, then clears when the snapshot lands.
    tmux send-keys -t "$session" Escape
    sleep 0.25
    screen=$(capture "$session")
    if ! printf '%s\n' "$screen" | grep -q "Storage slow.*discover"; then
        loud_fail "${delay}ms storage delay: thresholded discover feedback missing"
    fi

    loaded=0
    for _ in $(seq 1 160); do
        screen=$(capture "$session")
        if printf '%s\n' "$screen" | grep -q "slow-storage-probe"; then
            loaded=1
            break
        fi
        sleep 0.05
    done
    if (( loaded == 0 )); then
        loud_fail "${delay}ms storage delay: async snapshot never appeared"
    fi
    if printf '%s\n' "$screen" | grep -q "Storage slow"; then
        loud_fail "${delay}ms storage delay: phase feedback did not clear after load"
    fi

    quit_start=$(now_ms)
    tmux send-keys -t "$session" q
    for _ in $(seq 1 50); do
        if ! tmux has-session -t "$session" 2>/dev/null; then
            break
        fi
        sleep 0.01
    done
    quit_ms=$(( $(now_ms) - quit_start ))
    if tmux has-session -t "$session" 2>/dev/null || (( quit_ms >= 250 )); then
        loud_fail "${delay}ms storage delay: shutdown ${quit_ms}ms exceeded 250ms"
    fi
    echo "MEASURE: delay=${delay}ms first=${first_ms}ms help=${key_ms}ms quit=${quit_ms}ms"
done

echo "PASS: real TUI first frame/input remain responsive under 1-5s storage delay"
