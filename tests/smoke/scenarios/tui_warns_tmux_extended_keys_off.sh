#!/usr/bin/env bash
# Scenario: tui_warns_tmux_extended_keys_off (feat-wg-detect-extkeys)
#
# Drives the real `wg tui` inside an isolated tmux server and verifies the
# startup warning only appears when tmux's global `extended-keys` option is off.

set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot validate extended-keys warning"
fi

scratch="$(make_scratch)"
(cd "$scratch" && wg init --no-agency >/dev/null)
graph_dir="$(graph_dir_in "$scratch")" || loud_fail "no .wg dir under $scratch after wg init"

sock="wg-extkeys-${RANDOM}-$$"
session="wg-extkeys"
warning="tmux extended-keys is off"
remedy="set -g extended-keys on"
restart="tmux kill-server"

cleanup_tmux() {
    tmux -L "$sock" kill-server >/dev/null 2>&1 || true
}
add_cleanup_hook cleanup_tmux

tmux -L "$sock" new-session -d -s "$session" -x 100 -y 30 "sleep 60"

if ! tmux -L "$sock" show-options -gqv extended-keys >/dev/null 2>&1; then
    loud_skip "TMUX OPTION UNSUPPORTED" "this tmux does not expose the extended-keys option"
fi

run_case() {
    local value="$1"
    local log="$2"
    local sess="$session-$value"

    : >"$log"
    tmux -L "$sock" set-option -g extended-keys "$value" >/dev/null
    tmux -L "$sock" kill-session -t "$sess" >/dev/null 2>&1 || true
    tmux -L "$sock" new-session -d -s "$sess" -x 100 -y 30 \
        "cd '$scratch' && TERM=xterm-256color WG_DIR='$graph_dir' wg --dir '$graph_dir' tui 2>'$log'"

    for _ in $(seq 1 30); do
        if [[ "$value" == "off" && -s "$log" ]]; then
            break
        fi
        sleep 0.1
    done
    tmux -L "$sock" kill-session -t "$sess" >/dev/null 2>&1 || true
}

off_log="$scratch/off.stderr"
on_log="$scratch/on.stderr"

run_case off "$off_log"
run_case on "$on_log"

off_text="$(cat "$off_log")"
on_text="$(cat "$on_log")"

if [[ "$off_text" != *"$warning"* ]]; then
    loud_fail "expected warning with extended-keys off; stderr was: $off_text"
fi
if [[ "$off_text" != *"$remedy"* || "$off_text" != *"$restart"* ]]; then
    loud_fail "warning did not include exact remedy and restart instruction; stderr was: $off_text"
fi
if [[ "$on_text" == *"$warning"* ]]; then
    loud_fail "unexpected warning with extended-keys on; stderr was: $on_text"
fi

echo "=== tui_warns_tmux_extended_keys_off: PASS ==="
