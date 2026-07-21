#!/usr/bin/env bash
# Real installed-binary mouse/touch flow for the Chat selector. SGR presses
# exercise the same terminal event stream Termux/tmux delivers to wg tui.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v tmux >/dev/null 2>&1 \
    || loud_skip "MISSING TMUX" "tmux is required for the real TUI mouse flow"
command -v python3 >/dev/null 2>&1 \
    || loud_skip "MISSING PYTHON3" "python3 is required for coordinate and graph assertions"

unset TMUX WG_DIR WG_TASK_ID WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER
scratch=$(make_scratch)
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
export TMUX_TMPDIR="$scratch/tmux"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$TMUX_TMPDIR" "$scratch/fakebin"
WG_BIN=$(command -v wg)

# New-chat creation persists a real route but never needs credentials.
for provider in claude codex pi opencode; do
    cat >"$scratch/fakebin/$provider" <<'SH'
#!/usr/bin/env bash
while IFS= read -r line; do printf 'FAKE_CHAT:%s\n' "$line"; done
SH
    chmod +x "$scratch/fakebin/$provider"
done
export PATH="$scratch/fakebin:$PATH"

project="$scratch/project"
graph="$project/.wg"
mkdir -p "$project"
"$WG_BIN" --dir "$graph" init --no-agency >/dev/null 2>&1 \
    || loud_fail "fixture init failed"
cat >"$graph/config.toml" <<'TOML'
[dispatcher]
model = "claude:opus"
TOML
for name in alpha beta; do
    "$WG_BIN" --dir "$graph" chat create --name "$name" --command cat >/dev/null 2>&1 \
        || loud_fail "failed to create $name fixture chat"
done
printf '%s\n' '{"active_coordinator_id":0,"right_panel_tab":"Chat","open_tabs":[".chat-0",".chat-1"],"active":".chat-0"}' \
    >"$graph/tui-state.json"

session="wgsmoke-chat-selector-mouse-$$"
cleanup_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
    "$WG_BIN" --dir "$graph" service stop >/dev/null 2>&1 || true
}
add_cleanup_hook cleanup_session

tmux new-session -d -s "$session" -x 100 -y 30 \
    "env PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TMUX_TMPDIR='$TMUX_TMPDIR' '$WG_BIN' --dir '$graph' tui"
tmux resize-window -t "$session" -x 100 -y 30
tmux set-option -t "$session" mouse on

capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
wait_screen() {
    local needle=$1 label=$2
    for _ in $(seq 1 200); do
        capture | grep -Fq "$needle" && return 0
        sleep 0.03
    done
    loud_fail "$label: $(capture | tr '\n' '|')"
}
wait_not_screen() {
    local needle=$1 label=$2
    for _ in $(seq 1 200); do
        ! capture | grep -Fq "$needle" && return 0
        sleep 0.03
    done
    loud_fail "$label: $(capture | tr '\n' '|')"
}
chat_count() {
    python3 - "$graph/graph.jsonl" <<'PY'
import json, sys
print(sum(1 for line in open(sys.argv[1], encoding="utf-8")
          if line.strip() and json.loads(line).get("id", "").startswith(".chat-")))
PY
}

# Locate a visible label. With selector_row=1, constrain the search to rows
# below the Choose Chat title so a graph row behind the modal cannot win.
label_xy() {
    local needle=$1 selector_row=${2:-0}
    capture | python3 -c '
import sys
needle=sys.argv[1]
selector=sys.argv[2]=="1"
rows=sys.stdin.read().splitlines()
start=0
if selector:
    titles=[i for i,row in enumerate(rows) if "Choose Chat" in row]
    if not titles: raise SystemExit(1)
    start=titles[-1]+1
for y in range(start, len(rows)):
    x=rows[y].find(needle)
    if x >= 0:
        print(x+1, y+1) # SGR coordinates are 1-based
        raise SystemExit(0)
raise SystemExit(1)
' "$needle" "$selector_row"
}
mouse_click_label() {
    local needle=$1 selector_row=${2:-0} xy x y
    xy=$(label_xy "$needle" "$selector_row") \
        || loud_fail "could not locate clickable '$needle': $(capture | tr '\n' '|')"
    x=${xy% *}; y=${xy#* }
    tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM' "$x" "$y")"
    tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sm' "$x" "$y")"
}
open_selector_by_mouse() {
    mouse_click_label 'Chat ▾'
    wait_screen 'Choose Chat' 'mouse click did not open Chat selector'
}

wait_screen 'Chat ▾  .chat-0' 'initial live chat did not render'

# Open and select an exact non-current row with a single pointer press.
open_selector_by_mouse
mouse_click_label '.chat-1' 1
wait_not_screen 'Choose Chat' 'row click did not close selector'
wait_screen 'Chat ▾  .chat-1' 'row click did not attach exact .chat-1 identity'

# The rendered New footer action must create exactly one fresh chat through
# the real launcher; mouse chooses the action, Enter accepts its default route.
open_selector_by_mouse
mouse_click_label '[+] New' 1
wait_screen '+ Add new...' 'New footer did not open the launcher'
tmux send-keys -t "$session" Enter
for _ in $(seq 1 200); do
    [[ "$(chat_count)" == 3 ]] && break
    sleep 0.03
done
[[ "$(chat_count)" == 3 ]] || loud_fail "New footer did not create exactly one chat"
wait_screen 'Chat ▾  .chat-2' 'fresh chat did not become the exact active identity'

# Cancel is itself a pointer target, not merely a printed keyboard hint.
open_selector_by_mouse
mouse_click_label '[Esc] Cancel' 1
wait_not_screen 'Choose Chat' 'Cancel footer did not dismiss selector'
wait_screen 'Chat ▾  .chat-2' 'Cancel changed the active chat identity'
[[ "$(chat_count)" == 3 ]] || loud_fail "Cancel mutated chat count"

# Close targets the highlighted identity and reaches the normal destructive
# confirmation. Archive is used so the final graph transition is observable.
"$WG_BIN" --dir "$graph" service start >/dev/null 2>&1 \
    || loud_fail "daemon failed to start for confirmed Archive"
open_selector_by_mouse
mouse_click_label '[−] Close' 1
wait_screen 'Close Chat' 'Close footer did not open lifecycle choices'
wait_screen '.chat-2' 'Close footer targeted the wrong chat'
tmux send-keys -t "$session" a
wait_screen 'Confirm Archive chat' 'Close flow did not reach destructive confirmation'
tmux send-keys -t "$session" y
for _ in $(seq 1 200); do
    if python3 - "$graph/graph.jsonl" <<'PY' >/dev/null 2>&1
import json, sys
rows=[json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
row=next(r for r in rows if r.get("id")==".chat-2")
assert row.get("status")=="done" and "archived" in row.get("tags", []), row
PY
    then break; fi
    sleep 0.03
done
if ! python3 - "$graph/graph.jsonl" <<'PY'
import json, sys
rows=[json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if line.strip()]
row=next(r for r in rows if r.get("id")==".chat-2")
assert row.get("status")=="done" and "archived" in row.get("tags", []), row
PY
then
    loud_fail "confirmed Close/Archive did not transition exact .chat-2"
fi

# Keyboard remains authoritative after all pointer actions. Depending on
# whether the post-Archive handoff has already focused the child PTY, `~` is
# either direct or needs the documented Ctrl+O command-mode escape.
tmux send-keys -t "$session" '~'
for _ in $(seq 1 50); do
    capture | grep -Fq 'Choose Chat' && break
    sleep 0.02
done
if ! capture | grep -Fq 'Choose Chat'; then
    tmux send-keys -t "$session" C-o
    sleep 0.1
    tmux send-keys -t "$session" '~'
fi
wait_screen 'Choose Chat' 'keyboard did not open selector after mouse flow'
tmux send-keys -t "$session" Down Enter
wait_not_screen 'Choose Chat' 'keyboard Enter did not activate selector row'
wait_screen 'Chat ▾' 'keyboard selection did not return to a live Chat context'

echo "PASS: real tmux mouse flow opened Chat selector, selected exact row, created via New, canceled, and confirmed Close; keyboard flow remains live"
exit 0
