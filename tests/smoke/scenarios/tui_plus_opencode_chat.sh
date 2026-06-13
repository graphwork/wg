#!/usr/bin/env bash
# Scenario: tui_plus_opencode_chat (fix-real-tui)
#
# Drives the real `wg tui` plus-menu path in tmux. The regression this pins
# was missed by launcher state/unit tests: with an embedded chat PTY focused,
# pressing `+` was forwarded to the child process instead of opening the
# user-facing new-chat dialog.
#
# Flow:
#   1. Create a scratch WG project with a custom-command `cat` chat.
#   2. Launch `wg tui`, which focuses that chat PTY.
#   3. Press `+` and require the real launcher to render with `opencode`
#      visible in the Add-new executor choices.
#   4. Select `opencode`, type an OpenCode/OpenRouter route, launch, and
#      verify the new `.chat-1` metadata routes to OpenCode with no endpoint.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg tui plus-menu flow"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-plus-opencode-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session

fake_home="$scratch/home"
fake_global="$scratch/global"
mkdir -p "$fake_home/.config" "$fake_global"

project="$scratch/project"
mkdir -p "$project"
cd "$project"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" WG_GLOBAL_DIR="$fake_global" \
        wg "$@"
}

if ! run_wg init --no-agency -m claude:opus >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -20 init.log)"
fi

# Existing live PTY chat: this is the state where pre-fix `+` was swallowed
# by the embedded child. `cat` keeps the smoke credential-free.
if ! run_wg chat new --name base --command cat --json >base-chat.json 2>&1; then
    loud_fail "creating base cat chat failed: $(cat base-chat.json)"
fi

screen_text() {
    tmux capture-pane -t "$session" -p -S -80 2>/dev/null || true
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
    "cd '$project' && env HOME='$fake_home' XDG_CONFIG_HOME='$fake_home/.config' WG_GLOBAL_DIR='$fake_global' wg tui"

wait_screen_contains "[PTY]" "focused base chat PTY"
wait_screen_contains ".chat-0" "base chat tab"

# The actual user-facing regression: plain `+` must open the launcher even
# while the chat PTY owns focus.
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

for _ in $(seq 1 80); do
    if grep -q '"id":"\.chat-1"' .wg/graph.jsonl 2>/dev/null; then
        break
    fi
    sleep 0.25
done

python3 - "$route" <<'PY'
import json
import sys
from pathlib import Path

route = sys.argv[1]
graph = Path(".wg/graph.jsonl")
rows = [
    json.loads(line)
    for line in graph.read_text().splitlines()
    if line.strip()
]
chat = next((row for row in rows if row.get("id") == ".chat-1"), None)
if chat is None:
    raise SystemExit(f".chat-1 was not created by the TUI plus menu. graph:\n{graph.read_text()}")

if chat.get("executor_preset_name") != "opencode":
    raise SystemExit(f"expected executor_preset_name=opencode, got {chat.get('executor_preset_name')!r}: {chat}")
if chat.get("model") != route:
    raise SystemExit(f"expected model={route!r}, got {chat.get('model')!r}: {chat}")
if "endpoint" in chat or "endpoint_override" in chat:
    raise SystemExit(f"opencode chat must not persist an endpoint override in graph row: {chat}")

argv = chat.get("command_argv") or []
expected = ["opencode", "--model", "openrouter/stepfun/step-3.7-flash"]
if argv != expected:
    raise SystemExit(f"expected normalized OpenCode argv {expected!r}, got {argv!r}: {chat}")

state_path = Path(".wg/service/coordinator-state-1.json")
if not state_path.exists():
    raise SystemExit(f"missing {state_path}")
state = json.loads(state_path.read_text())
if state.get("executor_override") != "opencode":
    raise SystemExit(f"expected coordinator executor_override=opencode, got {state!r}")
if state.get("model_override") != route:
    raise SystemExit(f"expected coordinator model_override={route!r}, got {state!r}")
if state.get("endpoint_override") not in (None, ""):
    raise SystemExit(f"opencode coordinator state must not carry endpoint_override: {state!r}")
PY

echo "PASS: TUI '+' menu exposes opencode and creates .chat-1 with opencode OpenRouter metadata and no endpoint"
exit 0
