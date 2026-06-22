#!/usr/bin/env bash
# Scenario: tui_new_chat_focus_sticks (fix-tui-new-2)
#
# Drive the real TUI launcher flow in tmux, create a new custom-command chat,
# and verify the TUI's live dump switches from the previously-active chat to
# the newly-created chat and stays there across several refresh ticks.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive wg tui"
fi

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 not on PATH; cannot parse tui-dump JSON"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-new-chat-focus-$$"

kill_tmux_sessions() {
    tmux kill-session -t "$session" 2>/dev/null || true
    local project_tag
    project_tag=$(basename "$scratch" | tr '.:' '--')
    tmux ls 2>/dev/null \
        | awk -F: -v tag="wg-chat-${project_tag}-" '$1 ~ "^"tag {print $1}' \
        | while read -r s; do
            tmux kill-session -t "$s" 2>/dev/null || true
        done
}
add_cleanup_hook kill_tmux_sessions

cd "$scratch"

if ! wg init --executor shell >init.log 2>&1; then
    loud_fail "wg init --executor shell failed: $(tail -5 init.log)"
fi

if ! wg chat new --name previous --command "cat" >chat0.log 2>&1; then
    loud_fail "create initial cat chat failed: $(cat chat0.log)"
fi

start_wg_daemon "$scratch" --max-agents 1
graph_dir="$WG_SMOKE_DAEMON_DIR"

tui_dump_json() {
    wg --json tui-dump 2>/dev/null
}

tui_field() {
    local field="$1"
    tui_dump_json | python3 -c "
import json, sys
try:
    print(json.load(sys.stdin).get('$field', ''))
except Exception:
    pass
" 2>/dev/null
}

tui_text() {
    tui_field text
}

tui_cid() {
    tui_field coordinator_id
}

tui_input_mode() {
    tui_field input_mode
}

latest_chat_id() {
    python3 - "$graph_dir/graph.jsonl" <<'PY'
import json, re, sys
from pathlib import Path
path = Path(sys.argv[1])
best = None
for line in path.read_text().splitlines():
    if not line.strip():
        continue
    try:
        row = json.loads(line)
    except json.JSONDecodeError:
        continue
    task_id = row.get("id", "")
    m = re.fullmatch(r"\.chat-(\d+)", task_id)
    if not m:
        continue
    cid = int(m.group(1))
    if best is None or cid > best[0]:
        best = (cid, task_id)
print(best[1] if best else "")
PY
}

wait_for_tui_sock() {
    for _ in $(seq 1 30); do
        if [[ -S "$graph_dir/service/tui.sock" ]]; then
            return 0
        fi
        sleep 0.5
    done
    return 1
}

wait_for_input_mode() {
    local want="$1" tries="${2:-30}"
    for _ in $(seq 1 "$tries"); do
        if [[ "$(tui_input_mode)" == "$want" ]]; then
            return 0
        fi
        sleep 0.25
    done
    return 1
}

tmux new-session -d -s "$session" -x 180 -y 50 "wg tui"

if ! wait_for_tui_sock; then
    loud_fail "wg tui did not create tui.sock within 15s"
fi

if ! wait_for_input_mode "Normal" 30; then
    loud_fail "TUI never reached Normal mode after launch (input_mode=$(tui_input_mode))"
fi

old_cid=""
for _ in $(seq 1 30); do
    old_cid="$(tui_cid)"
    if [[ -n "$old_cid" ]]; then
        break
    fi
    sleep 0.25
done
if [[ -z "$old_cid" ]]; then
    loud_fail "wg tui-dump never returned an initial coordinator_id"
fi
echo "initial active coordinator_id=${old_cid}"

# The custom-command chat starts in PTY mode. Toggle back to command mode so
# the launcher hotkey is handled by the TUI instead of being sent to `cat`.
if tui_text | grep -q '\[PTY\]'; then
    tmux send-keys -t "$session" "C-o"
    sleep 0.5
fi

tmux send-keys -t "$session" "n"
if ! wait_for_input_mode "Launcher" 30; then
    loud_fail "pressing n did not open the launcher (input_mode=$(tui_input_mode))"
fi

# Default launcher: codex, claude, + Add new. Choose Add new, then select
# Custom Command, type `cat` into the command/model field, and submit.
tmux send-keys -t "$session" "Down" "Down" "Enter"
sleep 0.3
tmux send-keys -t "$session" "Right" "Right" "Right"
sleep 0.2
tmux send-keys -t "$session" "Tab"
sleep 0.2
tmux send-keys -t "$session" -l "cat"
sleep 0.2
tmux send-keys -t "$session" "Enter"

new_chat_id=""
for _ in $(seq 1 60); do
    new_chat_id="$(latest_chat_id)"
    if [[ -n "$new_chat_id" && "$new_chat_id" != ".chat-${old_cid}" ]]; then
        break
    fi
    sleep 0.5
done
if [[ -z "$new_chat_id" || "$new_chat_id" == ".chat-${old_cid}" ]]; then
    loud_fail "launcher did not create a new .chat-N task; latest=${new_chat_id:-<none>} graph=$(cat "$graph_dir/graph.jsonl")"
fi
new_cid="${new_chat_id#.chat-}"
echo "launcher created ${new_chat_id} (cid=${new_cid})"

for _ in $(seq 1 60); do
    cur="$(tui_cid)"
    if [[ "$cur" == "$new_cid" ]]; then
        break
    fi
    sleep 0.5
done

cur="$(tui_cid)"
if [[ "$cur" != "$new_cid" ]]; then
    loud_fail "TUI active coordinator_id is ${cur:-<empty>}, expected newly-created cid ${new_cid}"
fi

for tick in 1 2 3 4 5; do
    sleep 1.1
    cur="$(tui_cid)"
    if [[ "$cur" != "$new_cid" ]]; then
        dump="$(tui_dump_json 2>&1 | head -40)"
        loud_fail "after refresh tick ${tick}, coordinator_id reverted to ${cur:-<empty>} (expected ${new_cid}); dump=${dump}"
    fi
done

echo "PASS: TUI launcher focused new chat ${new_chat_id} and kept coordinator_id=${new_cid} across refresh ticks"
exit 0
