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
[models.default]
model = "pi:openai-codex:gpt-5.6-sol"
reasoning = "high"
TOML

ptydump="$scratch/ptydump"
"$WG_BIN" --dir "$G" chat create --name drag-full --command cat >/dev/null
"$WG_BIN" --dir "$G" add "drag-detail-exact-id" -d "detail survives mouse layout" >/dev/null
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
        "cd '$scratch/project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_PTY_DUMP='$ptydump' TERMUX_VERSION=0.119 MOSH_CONNECTION='smoke 0 0' MOSH_SERVER_PID=4242 '$WG_BIN' --dir '$G' tui"
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
    local identity=$1 dock=$2 screen count
    screen=$(capture)
    count=$(grep -oF "Split" <<<"$screen" | wc -l | tr -d ' ')
    [[ "$count" == 1 ]] || loud_fail "Full must expose exactly one contextual Split fallback, got $count: $screen"
    grep -Fq "$identity" <<<"$screen" || loud_fail "Full lost exact inspector identity $identity: $screen"
    grep -Fq '↯  ⌁  ⌂' <<<"$screen" || loud_fail "Full lost the one contextual navigation row: $screen"
    case "$dock" in
      right)
        python3 -c 'import sys; r=sys.stdin.read().splitlines(); assert len(r)>=20 and all((x+" ")[0]=="│" for x in r), "Full Right lacks one complete visible left boundary"' <<<"$screen" \
          || loud_fail "Full Right boundary is not completely painted: $screen"
        ;;
      bottom)
        python3 -c 'import sys; r=sys.stdin.read().splitlines(); assert r and len(r[0])>=20 and set(r[0])=={"─"}, "Full Bottom lacks one complete visible top boundary"' <<<"$screen" \
          || loud_fail "Full Bottom boundary is not completely painted: $screen"
        ;;
      *) loud_fail "unsupported Full chrome assertion dock=$dock" ;;
    esac
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
    wait_screen "h:Left" "layout command did not open with exact dock labels"
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
assert_full_chrome ".chat-0" right
read -r full_pty_w full_pty_h <<<"$(tmux display-message -p -t "$inner" '#{pane_width} #{pane_height}')"
(( full_pty_w >= 110 && full_pty_h >= 20 )) \
    || loud_fail "Full Chat did not receive fullscreen PTY dimensions ($full_pty_w x $full_pty_h)"
if grep -aFq $'\033[<' "$ptydump".env.*.in.bin 2>/dev/null; then
    loud_fail "mouse drag leaked an SGR sequence into Chat PTY"
fi

# Regression gesture: derive the literal visible left Full boundary from the
# candidate's rendered cells, press it in Chat body (not the contextual row),
# and drag right. Graph must reveal continuously into bounded Right Split.
full_boundary_x=$(vertical_seam_x) || loud_fail "could not locate visible Full left boundary"
sgr 0 "$full_boundary_x" 12 M
sgr 32 $((full_boundary_x + 24)) 12 M
sgr 0 $((full_boundary_x + 24)) 12 m
wait_layout mode split
wait_layout dock right
right_boundary_pct=$(layout_field size_percent)
(( right_boundary_pct < 90 && right_boundary_pct >= 10 )) \
    || loud_fail "literal Right Full boundary drag did not resize: $right_boundary_pct"

# The visible Full Workspace glyph remains a one-tap fallback and preserves the
# exact desired Right ratio. It is no longer the only pointer escape.
open_layout; tmux send-keys -t "$session" f Enter
wait_layout mode full
wait_screen "↔ Split" "Full fallback row did not settle before Workspace tap"
read -r workspace_x workspace_y < <(context_control_coord ".chat-0" "⌂") \
    || loud_fail "Full Workspace fallback is not visible"
sgr 0 "$workspace_x" "$workspace_y" M
sgr 0 "$workspace_x" "$workspace_y" m
wait_layout mode split
wait_layout dock right
[[ $(layout_field size_percent) == "$right_boundary_pct" ]] \
    || loud_fail "Workspace pointer escape lost exact remembered ratio"

# Auto-wide resolves physically to Right while retaining Auto as desired state;
# its same rendered left boundary uses the same natural rightward inverse drag.
tmux send-keys -t "$session" 0
wait_screen ".chat-0" "Chat did not return after Workspace pointer escape"
# Workspace left command mode armed on Graph; selecting Chat consumes the
# destination key but retains command routing, so plain p opens Layout.
tmux send-keys -t "$session" p
wait_screen "h:Left" "layout command did not open from Workspace-restored Graph"
tmux send-keys -t "$session" a f Enter
wait_layout mode full
wait_layout dock auto
assert_full_chrome ".chat-0" right
full_boundary_x=$(vertical_seam_x) || loud_fail "Auto-wide Full left boundary missing"
sgr 0 "$full_boundary_x" 14 M
sgr 32 $((full_boundary_x + 16)) 14 M
sgr 0 $((full_boundary_x + 16)) 14 m
wait_layout mode split
wait_layout dock auto
auto_pct=$(layout_field size_percent)
(( auto_pct < right_boundary_pct && auto_pct >= 10 )) \
    || loud_fail "Auto-wide boundary drag did not naturally shrink: $auto_pct"

# Contextual ↔ Split remains a fallback: return to explicit Right Full and drag
# that visible handle. The body-boundary flow above does not depend on it.
open_layout; tmux send-keys -t "$session" l f Enter
wait_layout mode full
wait_layout dock right
wait_screen "↔ Split" "side Full restore handle is not visible"
read -r split_x split_y < <(context_control_coord ".chat-0" "↔ Split") \
    || loud_fail "could not locate contextual Split fallback"
sgr 0 "$split_x" "$split_y" M
sgr 32 $((split_x + 12)) "$split_y" M
sgr 0 $((split_x + 12)) "$split_y" m
wait_layout mode split
wait_layout dock right
side_pct=$(layout_field size_percent)
(( side_pct < auto_pct && side_pct >= 10 )) || loud_fail "visible side fallback drag did not resize: $side_pct"
side_pty_w=0 side_pty_h=0
for _ in $(seq 1 100); do
    read -r side_pty_w side_pty_h <<<"$(tmux display-message -p -t "$inner" '#{pane_width} #{pane_height}')"
    (( side_pty_w >= 20 && side_pty_w < full_pty_w && side_pty_h >= 20 )) && break
    sleep 0.03
done
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

# The literal visible left boundary is equally authoritative over Detail.
tmux send-keys -t "$session" C-o
sleep 0.05
tmux send-keys -t "$session" 1
wait_screen "$task_context" "Detail context did not render"
wait_screen "drag-detail-exact-id" "Detail identity changed"
open_layout; tmux send-keys -t "$session" l f Enter
wait_layout mode full
assert_full_chrome "drag-detail-exact-id" right
full_boundary_x=$(vertical_seam_x) || loud_fail "Full Detail left boundary missing"
sgr 0 "$full_boundary_x" 16 M
sgr 32 $((full_boundary_x + 14)) 16 M
sgr 0 $((full_boundary_x + 14)) 16 m
wait_layout mode split
wait_layout dock right

# Re-enter Right Full, directly switch Detail→Log, and perform the exact body
# boundary gesture there too. Neither direct control nor boundary may fall into
# log content or select the hidden Graph.
open_layout; tmux send-keys -t "$session" f Enter
wait_layout mode full
read -r log_x log_y < <(context_control_coord "drag-detail-exact-id" " Log ") \
    || loud_fail "Full Right Detail row has no direct Log control"
sgr 0 "$log_x" "$log_y" M
sgr 0 "$log_x" "$log_y" m
wait_screen "view=[Events]" "direct Detail→Log did not open before boundary test"
assert_full_chrome "drag-detail-exact-id" right
full_boundary_x=$(vertical_seam_x) || loud_fail "Full Log left boundary missing"
sgr 0 "$full_boundary_x" 18 M
sgr 32 $((full_boundary_x + 10)) 18 M
sgr 0 $((full_boundary_x + 10)) 18 m
wait_layout mode split
wait_layout dock right

# Exercise the natural stacked Bottom seam next. Pointer-down uses the graph
# row immediately above the contextual seam while entering Full.
tmux send-keys -t "$session" C-o
sleep 0.05
tmux send-keys -t "$session" 1
wait_screen "drag-detail-exact-id" "Detail did not return after Full Log boundary"
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
assert_full_chrome "drag-detail-exact-id" bottom
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

# Reverse-drag the literal visible top boundary down from Full; Bottom stays
# the intended dock and a bounded graph split becomes visible again.
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
assert_full_chrome ".chat-0" bottom
read -r restart_full_pty_w restart_full_pty_h <<<"$(tmux display-message -p -t "$inner" '#{pane_width} #{pane_height}')"
(( restart_full_pty_w >= 110 && restart_full_pty_h >= 20 )) \
    || loud_fail "restarted Full Chat has unusable PTY dimensions ($restart_full_pty_w x $restart_full_pty_h)"

# Tap (no drag) the visible top boundary after restart. It restores the
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

# At the resized Termux+mosh geometry, Auto resolves to stacked Bottom. Press
# the rendered top boundary and drag down to prove the natural mobile inverse.
open_layout; tmux send-keys -t "$session" a f Enter
wait_layout mode full
wait_layout dock auto
assert_full_chrome ".chat-0" bottom
sgr 0 50 1 M
sgr 32 50 7 M
sgr 0 50 7 m
wait_layout mode split
wait_layout dock auto
auto_narrow_pct=$(layout_field size_percent)
(( auto_narrow_pct < stacked_pct && auto_narrow_pct >= 10 )) \
    || loud_fail "Auto narrow/Termux boundary drag did not shrink naturally: $auto_narrow_pct"

if grep -aFq $'\033[<' "$ptydump".env.*.in.bin 2>/dev/null; then
    loud_fail "resize/drag leaked input to PTY"
fi
[[ $(tmux display-message -p -t "$inner" '#{pane_pid}') == "$chat_pid" ]] || loud_fail "resize respawned Chat"

echo "PASS: real Right/Auto-wide Chat, Detail, Log, stacked Bottom, and Auto-narrow Termux+mosh Full boundaries restore Graph; Workspace/Split fallbacks, restart, rotation, PTY identity/input confinement hold"
