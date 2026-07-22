#!/usr/bin/env bash
# Real tmux/SGR flow for same-frame Session Log header controls.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "cargo is required"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

scratch=$(make_scratch)
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
  WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
else
  export CARGO_TARGET_DIR="$scratch/candidate-target"
  (cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
  WG_BIN="$CARGO_TARGET_DIR/debug/wg"
fi
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"

export HOME="$scratch/home" XDG_CONFIG_HOME="$scratch/home/.config" WG_GLOBAL_DIR="$scratch/home/.wg"
unset TMUX TMUX_TMPDIR WG_DIR WG_TASK_ID WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$scratch/project"
G="$scratch/project/.wg"
"$WG_BIN" --dir "$G" init --no-agency >/dev/null
"$WG_BIN" --dir "$G" add "Clickable log task" --id clickable-log -d "pointer parity fixture" --no-place >/dev/null
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
p=sys.argv[1]; out=[]
for line in open(p):
    if not line.strip(): continue
    o=json.loads(line)
    if o.get("kind") == "task" and o.get("id") == "clickable-log":
        o["status"]="in-progress"; o["assigned"]="agent-new"; o["retry_count"]=1
        o["log"]=[
          {"timestamp":"2026-07-22T10:00:00Z","actor":"agent-old","message":"Claimed"},
          {"timestamp":"2026-07-22T10:01:00Z","actor":"agent-new","message":"Claimed"},
        ]
    out.append(json.dumps(o))
open(p,"w").write("\n".join(out)+"\n")
PY
mkdir -p "$G/service" "$G/agents/agent-old" "$G/agents/agent-new"
python3 - "$G/service/registry.json" "$$" <<'PY'
import json,sys
p,pid=sys.argv[1],int(sys.argv[2])
r={"next_agent_id":2,"agents":{
 "agent-old":{"id":"agent-old","pid":pid,"task_id":"clickable-log","executor":"pi","started_at":"2026-07-22T10:00:00Z","last_heartbeat":"2026-07-22T10:00:30Z","status":"failed","output_file":".wg/agents/agent-old/output.log","completed_at":"2026-07-22T10:00:30Z"},
 "agent-new":{"id":"agent-new","pid":pid,"task_id":"clickable-log","executor":"pi","started_at":"2026-07-22T10:01:00Z","last_heartbeat":"2026-07-22T10:01:30Z","status":"working","output_file":".wg/agents/agent-new/output.log"}
}}
open(p,"w").write(json.dumps(r))
PY
printf '%s\n' '{"type":"turn_end","message":{"content":[{"type":"text","text":"OLD_ATTEMPT"}]}}' >"$G/agents/agent-old/raw_stream.jsonl"
printf '%s\n' '{"type":"turn_end","message":{"content":[{"type":"text","text":"NEW_ATTEMPT"}]}}' >"$G/agents/agent-new/raw_stream.jsonl"
: >"$G/agents/agent-old/output.log"; : >"$G/agents/agent-new/output.log"
cat >"$G/tui-state.json" <<'JSON'
{"layout":{"dock":"right","size_percent":85,"mode":"split"},"right_panel_tab":"Detail"}
JSON

session="wg-log-header-clicks-$$"
cleanup_session() { tmux kill-session -t "$session" 2>/dev/null || true; }
add_cleanup_hook cleanup_session
capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
wait_screen() {
  local needle=$1 label=${2:-"screen missing $1"}
  for _ in $(seq 1 240); do capture | grep -Fq "$needle" && return 0; sleep 0.03; done
  loud_fail "$label: $(capture | tr '\n' '|')"
}
coord() {
  local needle=$1
  capture | python3 -c 'import sys
needle=sys.argv[1]
for y,row in enumerate(sys.stdin.read().splitlines(),1):
    x=row.find(needle)
    if x >= 0:
        print(x+1,y); raise SystemExit(0)
raise SystemExit(1)' "$needle"
}
click_text() {
  local needle=$1 offset=${2:-0} xy x y
  xy=$(coord "$needle") || loud_fail "click target missing: $needle: $(capture | tr '\n' '|')"
  read -r x y <<<"$xy"
  x=$((x + offset))
  tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM\033[<0;%s;%sm' "$x" "$y" "$x" "$y")"
}

# Select the real graph row, enter Session Log with the documented key, then
# click the actual view/value cells through the complete four-mode cycle.
tmux new-session -d -s "$session" -x 220 -y 40 \
  "cd '$scratch/project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' '$WG_BIN' --dir '$G' tui"
tmux set-option -t "$session" mouse on
tmux resize-window -t "$session" -x 220 -y 40
wait_screen "clickable-log"
click_text "clickable-log"
tmux send-keys -t "$session" 4
wait_screen "view=[Events]"
for transition in \
  'view=[Events]|view=[HighLevel]' \
  'view=[HighLevel]|view=[RawPretty]' \
  'view=[RawPretty]|view=[WgLog]' \
  'view=[WgLog]|view=[Events]'; do
  before=${transition%%|*}; after=${transition#*|}
  click_text "$before"
  wait_screen "$after" "clicking $before did not render $after"
done

# Let the asynchronous attempt snapshot settle before testing controls whose
# hit owner deliberately rejects a source change between paint and tap.
wait_screen "NEW_ATTEMPT"
sleep 0.3
# Every other retained wide label invokes its keyboard-equivalent method.
click_text "[s] summary" 5
wait_screen "summary=on" "summary label did not toggle summary mode"
click_text "[J] json" 4
wait_screen "json=on" "JSON label did not toggle JSON mode"
click_text "[{] older" 4
wait_screen "OLD_ATTEMPT" "older-attempt label did not pin the failed source"
click_text "[}] newer" 4
wait_screen "NEW_ATTEMPT" "newer-attempt label did not restore the live source"

# Phone/Termux width keeps all five compact controls complete and tappable.
tmux resize-window -t "$session" -x 40 -y 24
wait_screen "view=[Events]"
wait_screen "[s] [J] [{] [}]" "40-column compact Log controls were clipped"
click_text "view=[Events]"
wait_screen "view=[HighLevel]"
click_text "[s]" 1
# State was on from the wide click, so the compact click turns it back off.
tmux resize-window -t "$session" -x 220 -y 40
wait_screen "summary=off" "compact summary tap did not reach the same action"

# Repeat the mode sequence by keyboard after resize; pointer support must not
# alter the established `4` ownership or task/attempt identity.
for expected in RawPretty WgLog Events HighLevel; do
  tmux send-keys -t "$session" 4
  wait_screen "view=[$expected]" "keyboard 4 failed after pointer/resize flow"
done
wait_screen "agent=agent-new"

echo "PASS: Session Log same-frame view/summary/JSON/attempt controls click at wide and Termux widths; resize and key 4 retain exact ownership"
