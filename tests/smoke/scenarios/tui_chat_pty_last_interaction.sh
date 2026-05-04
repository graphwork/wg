#!/usr/bin/env bash
# Scenario: tui_chat_pty_last_interaction
#
# Drives the actual TUI chat-tab PTY flow: launch `wg tui` in tmux, focus an
# existing custom-command chat tab, type a message into the embedded PTY, and
# assert the backing chat task's last_interaction_at changes quickly and sorts
# ahead of a previously newer chat.
#
# Uses `cat` as the chat command so the user-visible PTY path is live without
# requiring LLM credentials.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive a PTY for the TUI"
fi

scratch=$(make_scratch)
session="wgsmoke-tui-chat-pty-lia-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session
cd "$scratch"

if ! wg init --executor shell >init.log 2>&1; then
    loud_fail "wg init --executor shell failed: $(tail -5 init.log)"
fi

if ! wg chat new --name active --command "cat" >chat0.log 2>&1; then
    loud_fail "create active cat chat failed: $(cat chat0.log)"
fi
sleep 1
if ! wg chat new --name newer --command "cat" >chat1.log 2>&1; then
    loud_fail "create newer cat chat failed: $(cat chat1.log)"
fi

read_lia() {
    local id="$1"
    python3 - "$id" <<'PY'
import json, sys
from pathlib import Path
want = sys.argv[1]
for line in Path(".wg/graph.jsonl").read_text().splitlines():
    if not line.strip():
        continue
    obj = json.loads(line)
    if obj.get("id") == want:
        print(obj.get("last_interaction_at") or "")
        break
PY
}

top_chat_by_activity() {
    python3 - <<'PY'
import json
from pathlib import Path
rows = []
for line in Path(".wg/graph.jsonl").read_text().splitlines():
    if not line.strip():
        continue
    obj = json.loads(line)
    if obj.get("kind") != "task":
        continue
    if "chat-loop" not in obj.get("tags", []):
        continue
    rows.append((obj.get("last_interaction_at") or obj.get("created_at") or "", obj["id"]))
rows.sort(reverse=True)
print(rows[0][1] if rows else "")
PY
}

before=$(read_lia .chat-0)
[[ -n "$before" ]] || loud_fail ".chat-0 missing last_interaction_at before TUI typing"

start_wg_daemon "$scratch" --max-agents 1
graph_dir="$WG_SMOKE_DAEMON_DIR"

tmux new-session -d -s "$session" -x 180 -y 50 "wg tui"
for _ in $(seq 1 30); do
    if [[ -S "$graph_dir/service/tui.sock" ]]; then
        break
    fi
    sleep 0.5
done
if [[ ! -S "$graph_dir/service/tui.sock" ]]; then
    loud_fail "wg tui did not create tui.sock within 15s"
fi

# Wait for the custom-command PTY to spawn and own focus. The first chat is
# active by default, so typing goes into .chat-0's embedded cat process.
sleep 2
payload="smoke lia $(date +%s)"
tmux send-keys -t "$session" "$payload" Enter

after=""
for _ in $(seq 1 20); do
    after=$(read_lia .chat-0)
    if [[ -n "$after" && "$after" != "$before" && "$after" > "$before" ]]; then
        break
    fi
    sleep 0.25
done

if [[ -z "$after" || "$after" == "$before" || "$after" < "$before" ]]; then
    loud_fail "TUI chat typing did not bump .chat-0 within 5s: before=$before after=$after graph=$(cat .wg/graph.jsonl)"
fi

top=$(top_chat_by_activity)
if [[ "$top" != ".chat-0" ]]; then
    loud_fail "typed-in TUI chat did not sort first by last_interaction_at within 5s: top=$top before=$before after=$after graph=$(cat .wg/graph.jsonl)"
fi

echo "BEFORE .chat-0 last_interaction_at: $before"
echo "TYPED via wg tui tmux session '$session': $payload"
echo "AFTER .chat-0 last_interaction_at:  $after"
echo "PASS: TUI chat PTY typing bumps last_interaction_at and bubbles active chat to top"
exit 0
