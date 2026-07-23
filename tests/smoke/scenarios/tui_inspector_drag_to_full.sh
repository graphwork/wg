#!/usr/bin/env bash
# Real tmux/SGR mouse flow for drag-to-Full on both side and stacked inspector
# seams. Uses a persistent command Chat so PTY identity/input confinement are
# observable across Full, restore, resize, Detail, and TUI restart.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "cargo is required"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
cd "$REPO_ROOT"
CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg
WG_BIN="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg"
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"

scratch=$(make_scratch)
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
# Keep the normal default tmux socket. The embedded attach client deliberately
# unsets TMUX, so a custom TMUX_TMPDIR would make create and attach see
# different servers under an outer-tmux smoke harness.
unset TMUX_TMPDIR
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$scratch/project"
G="$scratch/project/.wg"
"$WG_BIN" --dir "$G" init --no-agency >/dev/null
cat >"$G/config.toml" <<'TOML'
[dispatcher]
model = "claude:opus"
TOML

ptydump="$scratch/ptydump"
"$WG_BIN" --dir "$G" chat create --name drag-full --command cat >/dev/null
"$WG_BIN" --dir "$G" add "drag-detail-exact-id" -d "detail survives mouse layout" --no-place >/dev/null
cat >"$G/tui-state.json" <<'JSON'
{"layout":{"dock":"right","size_percent":60,"mode":"split"},"active_coordinator_id":0,"right_panel_tab":"Chat","open_tabs":[".chat-0"],"active":".chat-0"}
JSON

session="wg-inspector-drag-full-$$"
inner=""
cleanup_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
    [[ -z "$inner" ]] || tmux kill-session -t "$inner" 2>/dev/null || true
}
add_cleanup_hook cleanup_session

start_tui() {
    tmux new-session -d -s "$session" -x 120 -y 32 \
        "cd '$scratch/project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_PTY_DUMP='$ptydump' '$WG_BIN' --dir '$G' tui"
    tmux resize-window -t "$session" -x 120 -y 32
    tmux set-option -t "$session" mouse on
}
capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
wait_screen() {
    local needle=$1 label=${2:-"screen missing $1"}
    for _ in $(seq 1 240); do
        capture | grep -Fq "$needle" && return 0
        sleep 0.03
    done
    loud_fail "$label: $(capture | tr '\n' '|')"
}
layout_field() {
    python3 - "$G/tui-state.json" "$1" <<'PY'
import json, sys
print(str(json.load(open(sys.argv[1], encoding="utf-8"))["layout"][sys.argv[2]]).lower())
PY
}
wait_layout() {
    local field=$1 expected=$2
    for _ in $(seq 1 200); do
        [[ $(layout_field "$field" 2>/dev/null || true) == "$expected" ]] && return 0
        sleep 0.03
    done
    loud_fail "layout $field did not become $expected"
}
sgr() {
    local code=$1 x=$2 y=$3 suffix=$4
    tmux send-keys -t "$session" -l "$(printf '\033[<%s;%s;%s%s' "$code" "$x" "$y" "$suffix")"
}
vertical_seam_x() {
    capture | python3 -c '
import collections, sys
rows=sys.stdin.read().splitlines()
c=collections.Counter()
for row in rows:
    for x,ch in enumerate(row):
        if ch=="│": c[x]+=1
if not c: raise SystemExit(1)
x,n=c.most_common(1)[0]
if n < 8: raise SystemExit(1)
print(x+1)
'
}
context_row_y() {
    local label=$1
    capture | python3 -c '
import sys
needle=sys.argv[1]
for y,row in enumerate(sys.stdin.read().splitlines()):
    if needle in row:
        print(y+1)
        raise SystemExit(0)
raise SystemExit(1)
' "$label"
}
assert_full_chrome() {
    local identity=$1 screen count
    screen=$(capture)
    count=$(grep -oF "Split" <<<"$screen" | wc -l | tr -d ' ')
    [[ "$count" == 1 ]] || loud_fail "Full must expose exactly one visible contextual restore handle, got $count: $screen"
    grep -Fq "$identity" <<<"$screen" || loud_fail "Full lost exact inspector identity $identity: $screen"
    grep -Fq '↯  ⌁  ⌂' <<<"$screen" || loud_fail "Full lost the one contextual navigation row: $screen"
    # With graph width zero there is no live seam and no outer frame/strip.
    ! grep -qF '│' <<<"$screen" || loud_fail "Full retained a vertical seam/frame: $screen"
    ! grep -qE '^[[:space:]]*[┌┐└┘]' <<<"$screen" || loud_fail "Full retained outer frame corners: $screen"
}
context_control_coord() {
    local identity=$1 label=$2
    capture | python3 -c '
import sys
identity,label=sys.argv[1:]
for y,row in enumerate(sys.stdin.read().splitlines()):
    if identity in row:
        x=row.find(label)
        if x >= 0:
            print(x + max(1, len(label)//2) + 1, y + 1)
            raise SystemExit(0)
raise SystemExit(1)
' "$identity" "$label"
}
wait_screen_absent() {
    local needle=$1 label=$2
    for _ in $(seq 1 200); do
        ! capture | grep -Fq "$needle" && return 0
        sleep 0.03
    done
    loud_fail "$label: $(capture | tr '\n' '|')"
}
open_layout() {
    tmux send-keys -t "$session" C-o
    sleep 0.05
    tmux send-keys -t "$session" p
    wait_screen "h/j/k/l dock" "layout command did not open"
}

chat_context="↯  ⌁  ⌂  .chat-0"
task_context="↯  ⌁  ⌂  drag-detail-exact"

start_tui
wait_screen "$chat_context" "Chat context did not render"
for _ in $(seq 1 200); do
    inner=$(tmux list-sessions -F '#S' 2>/dev/null | grep -E '^wg-chat-.*-chat-0$' | head -1 || true)
    [[ -n "$inner" ]] && break
    sleep 0.03
done
[[ -n "$inner" ]] || loud_fail "persistent inner Chat tmux session missing"
chat_pid=$(tmux display-message -p -t "$inner" '#{pane_pid}')
[[ -n "$chat_pid" ]] || loud_fail "could not record Chat pane identity"

# Right-side split: press its one live seam, cross the 96% snap at the physical
# left edge, jitter back below it, then release. Full must remain latched.
seam_x=$(vertical_seam_x) || loud_fail "could not locate the live side seam: $(capture | tr '\n' '|')"
sgr 0 "$seam_x" 12 M
sgr 32 1 12 M
sgr 32 8 12 M
sgr 0 8 12 m
wait_layout mode full
[[ $(layout_field dock) == right ]] || loud_fail "side drag changed desired dock"
[[ $(layout_field size_percent) == 90 ]] || loud_fail "Full did not retain bounded 90% split"
wait_screen ".chat-0" "Chat context vanished in Full"
assert_full_chrome ".chat-0"
read -r full_pty_w full_pty_h <<<"$(tmux display-message -p -t "$inner" '#{pane_width} #{pane_height}')"
(( full_pty_w >= 110 && full_pty_h >= 20 )) \
    || loud_fail "Full Chat did not receive fullscreen PTY dimensions ($full_pty_w x $full_pty_h)"
if grep -aFq $'\033[<' "$ptydump".env.*.in.bin 2>/dev/null; then
    loud_fail "mouse drag leaked an SGR sequence into Chat PTY"
fi

# The visible Full Workspace glyph is a one-tap atomic escape: Graph becomes
# visible and the exact desired Right/90 split survives. No keyboard recovery.
sgr 0 8 1 M
sgr 0 8 1 m
wait_layout mode split
wait_layout dock right
[[ $(layout_field size_percent) == 90 ]] || loud_fail "Workspace pointer escape lost exact remembered ratio"

# Return to the exact Chat surface, enter Full again, and reverse-drag the
# visible contextual handle rightward. The handle — not an invisible edge —
# restores and resizes the remembered split.
tmux send-keys -t "$session" 0
wait_screen ".chat-0" "Chat did not return after Workspace pointer escape"
# Workspace escape deliberately left host focus on Graph, so plain p (not a
# second Ctrl+O toggle back into the child) opens the layout row.
tmux send-keys -t "$session" p
wait_screen "h/j/k/l dock" "layout command did not open from restored Graph"
tmux send-keys -t "$session" f Enter
wait_layout mode full
wait_screen "↔ Split" "side Full restore handle is not visible"
sgr 0 12 1 M
sgr 32 32 1 M
sgr 0 32 1 m
wait_layout mode split
wait_layout dock right
side_pct=$(layout_field size_percent)
(( side_pct < 90 && side_pct >= 10 )) || loud_fail "visible side reverse drag did not resize: $side_pct"
read -r side_pty_w side_pty_h <<<"$(tmux display-message -p -t "$inner" '#{pane_width} #{pane_height}')"
(( side_pty_w >= 20 && side_pty_w < full_pty_w && side_pty_h >= 20 )) \
    || loud_fail "side restore did not resize Chat PTY from Full ($full_pty_w x $full_pty_h → $side_pty_w x $side_pty_h)"

# Type through the same Chat pane; pointer escape must not steal printable
# input or respawn/duplicate the child.
tmux send-keys -t "$session" -l PTY_AFTER_SIDE
tmux send-keys -t "$session" Enter
wait_screen "PTY_AFTER_SIDE" "restored Chat PTY did not receive confined input"
python3 - "$ptydump" <<'PY'
import glob, sys
payload=b''.join(open(p,'rb').read() for p in glob.glob(sys.argv[1]+'.env.*.in.bin'))
assert b'PTY_AFTER_SIDE\r' in payload, payload
assert b'\x1b[<' not in payload, payload
PY
[[ $(tmux display-message -p -t "$inner" '#{pane_pid}') == "$chat_pid" ]] || loud_fail "side Full/restore respawned Chat"

# Exercise Detail and a stacked Bottom seam. Pointer-down uses the graph row
# immediately above the contextual seam because visible contextual controls win
# on the seam row itself.
tmux send-keys -t "$session" C-o
sleep 0.05
tmux send-keys -t "$session" 1
wait_screen "$task_context" "Detail context did not render"
wait_screen "drag-detail-exact-id" "Detail identity changed"
open_layout
tmux send-keys -t "$session" j
# Normalize this independent Bottom case to the largest bounded split before
# crossing the separate 96% Full threshold.
for _ in $(seq 1 20); do tmux send-keys -t "$session" +; done
tmux send-keys -t "$session" Enter
wait_layout dock bottom
wait_layout mode split
wait_screen "$task_context" "stacked contextual seam did not render"
context_y=$(context_row_y "$task_context") || loud_fail "could not locate stacked seam"
start_y=$((context_y - 1))
(( start_y >= 1 )) || loud_fail "invalid stacked seam coordinate $start_y"
sgr 0 60 "$start_y" M
sgr 32 60 1 M
sgr 32 60 4 M
sgr 0 60 4 m
wait_layout mode full
[[ $(layout_field dock) == bottom ]] || loud_fail "stacked drag changed desired dock"
[[ $(layout_field size_percent) == 90 ]] || loud_fail "stacked Full lost bounded split"
wait_screen "drag-detail-exact-id" "Detail context vanished in Full"
assert_full_chrome "drag-detail-exact-id"
wait_screen "↕ Split" "stacked Full restore handle is not visible"

# Detail → current-attempt Log → the same Detail is a direct one-tap local
# switch in the contextual row; the status/token metadata is not involved.
read -r log_x log_y < <(context_control_coord "drag-detail-exact-id" " Log ") \
    || loud_fail "Full Detail row has no direct Log control: $(capture | tr '\n' '|')"
sgr 0 "$log_x" "$log_y" M
sgr 0 "$log_x" "$log_y" m
wait_screen "view=[Events]" "direct Detail→Log did not open the current-attempt tail"
read -r detail_x detail_y < <(context_control_coord "drag-detail-exact-id" " Detail ") \
    || loud_fail "Full Log row has no direct Detail control: $(capture | tr '\n' '|')"
sgr 0 "$detail_x" "$detail_y" M
sgr 0 "$detail_x" "$detail_y" m
wait_screen_absent "view=[Events]" "direct Log→Detail did not return to Detail"
wait_screen "drag-detail-exact-id" "direct Log→Detail changed task identity"

# Reverse-drag the visible vertical handle down from Full; Bottom stays the
# intended dock and a bounded graph split becomes visible again.
sgr 0 12 1 M
sgr 32 12 9 M
sgr 0 12 9 m
wait_layout mode split
wait_layout dock bottom
stacked_pct=$(layout_field size_percent)
(( stacked_pct < 90 && stacked_pct >= 10 )) || loud_fail "visible stacked reverse drag did not resize: $stacked_pct"

# Re-enter Full so restart proves the escape remains visible after persisted
# state reload, then kill only the outer TUI. The inner Chat process survives.
open_layout
tmux send-keys -t "$session" f Enter
wait_layout mode full

# Kill only the outer TUI. Full is already atomically persisted; the inner Chat
# tmux process must survive and a fresh TUI must reload Full without respawn.
tmux kill-session -t "$session"
start_tui
wait_layout mode full
wait_screen ".chat-0" "restarted TUI did not reload the full inspector"
[[ $(tmux display-message -p -t "$inner" '#{pane_pid}') == "$chat_pid" ]] || loud_fail "TUI restart respawned persistent Chat"
assert_full_chrome ".chat-0"
read -r restart_full_pty_w restart_full_pty_h <<<"$(tmux display-message -p -t "$inner" '#{pane_width} #{pane_height}')"
(( restart_full_pty_w >= 110 && restart_full_pty_h >= 20 )) \
    || loud_fail "restarted Full Chat has unusable PTY dimensions ($restart_full_pty_w x $restart_full_pty_h)"

# Tap (no drag) the visible vertical handle after restart. It restores the
# exact remembered Bottom split, then a real seam drag is resized mid-gesture.
sgr 0 12 1 M
sgr 0 12 1 m
wait_layout mode split
wait_layout dock bottom
[[ $(layout_field size_percent) == "$stacked_pct" ]] || loud_fail "restart pointer restore lost remembered ratio $stacked_pct"
wait_screen "$chat_context" "stacked Chat context missing after restore"
stacked_pty_w=0 stacked_pty_h=0
for _ in $(seq 1 100); do
    read -r stacked_pty_w stacked_pty_h <<<"$(tmux display-message -p -t "$inner" '#{pane_width} #{pane_height}')"
    (( stacked_pty_w >= 110 && stacked_pty_h >= 3 && stacked_pty_h < restart_full_pty_h )) && break
    sleep 0.03
done
(( stacked_pty_w >= 110 && stacked_pty_h >= 3 && stacked_pty_h < restart_full_pty_h )) \
    || loud_fail "Bottom restore did not resize Chat PTY from Full ($restart_full_pty_w x $restart_full_pty_h → $stacked_pty_w x $stacked_pty_h)"
context_y=$(context_row_y "$chat_context") || loud_fail "could not locate restored stacked seam"
start_y=$((context_y - 1))
sgr 0 60 "$start_y" M
tmux resize-window -t "$session" -x 100 -y 28
sleep 0.15
sgr 32 50 1 M
sgr 0 50 1 m
sleep 0.15
[[ $(layout_field mode) == split ]] || loud_fail "stale post-resize drag snapped layout"
[[ $(layout_field dock) == bottom ]] || loud_fail "stale post-resize drag changed dock"
[[ $(layout_field size_percent) == "$stacked_pct" ]] || loud_fail "stale post-resize drag changed ratio"
if grep -aFq $'\033[<' "$ptydump".env.*.in.bin 2>/dev/null; then
    loud_fail "resize/drag leaked input to PTY"
fi
[[ $(tmux display-message -p -t "$inner" '#{pane_pid}') == "$chat_pid" ]] || loud_fail "resize respawned Chat"

echo "PASS: real side+stacked Full exposes pointer Workspace/handle escape and direct Detail↔Log; restart, rotation, PTY identity/input confinement hold"
