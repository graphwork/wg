#!/usr/bin/env bash
# Scenario: tui_chat_model_picker_pty
#
# Drives the real WG TUI in tmux and opens the chat-scoped `/model` picker
# through the human slash-command flow. Selecting a model must dispatch the
# same `wg chat model` / SetChatExecutor path as the CLI verb, which writes
# per-chat CoordinatorState while the daemon is running.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive a PTY for the TUI"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-model-picker-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session

cd "$scratch"

if ! wg init --no-agency --executor shell >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -20 init.log)"
fi

start_wg_daemon "$scratch" --max-agents 1
graph_dir="$WG_SMOKE_DAEMON_DIR"

if ! wg chat create --name base >chat-create.log 2>&1; then
    loud_fail "wg chat create failed: $(tail -20 chat-create.log)"
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

tmux new-session -d -s "$session" -x 180 -y 50 "cd '$scratch' && wg tui"

wait_screen_contains ".chat-0" "base chat tab"

# If the embedded chat PTY has focus, Ctrl+O shifts to WG command mode so
# slash commands are handled by WG rather than forwarded to the child.
tmux send-keys -t "$session" C-o
sleep 0.2

# Human flow: type the slash command, filter the picker, accept the highlighted
# row. Pre-fix this stayed in chat search / message flow and never rendered a
# `/model` picker or wrote CoordinatorState.
tmux send-keys -t "$session" "/"
tmux send-keys -t "$session" -l "model"
tmux send-keys -t "$session" Enter
wait_screen_contains "/model" "model picker title"
wait_screen_contains "Search:" "model picker search field"

tmux send-keys -t "$session" -l "minimax m3"
wait_screen_contains "minimax/minimax-m3" "filtered minimax model suggestion"
tmux send-keys -t "$session" Enter

state_file="$graph_dir/service/coordinator-state-0.json"
for _ in $(seq 1 80); do
    if [[ -f "$state_file" ]] && grep -q '"model_override"' "$state_file"; then
        break
    fi
    sleep 0.25
done

if [[ ! -f "$state_file" ]]; then
    loud_fail "model picker did not write $state_file"
fi

if ! python3 - "$state_file" <<'PY'
import json
import sys
from pathlib import Path

state = json.loads(Path(sys.argv[1]).read_text())
expected_model = "minimax/minimax-m3"
expected_executor = "claude"
model = state.get("model_override")
executor = state.get("executor_override")
if model != expected_model:
    raise SystemExit(f"expected model_override={expected_model!r}, got {model!r}: {state}")
if executor != expected_executor:
    raise SystemExit(f"expected executor_override={expected_executor!r} from SetChatExecutor, got {executor!r}: {state}")
PY
then
    loud_fail "model picker wrote unexpected CoordinatorState: $(cat "$state_file")"
fi

wait_screen_contains "model set to minimax/minimax-m3" "success toast"

echo "PASS: /model picker selected minimax/minimax-m3 and SetChatExecutor wrote $state_file"
exit 0
