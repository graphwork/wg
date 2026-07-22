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
    local label=$1 screen count
    screen=$(capture)
    count=$(grep -oF "$label" <<<"$screen" | wc -l | tr -d ' ')
    [[ "$count" == 1 ]] || loud_fail "Full must retain exactly one contextual row ($label), got $count: $screen"
    # With graph width zero there is no live seam and no outer frame/strip.
    ! grep -qF '│' <<<"$screen" || loud_fail "Full retained a vertical seam/frame: $screen"
    ! grep -qE '^[[:space:]]*[┌┐└┘]' <<<"$screen" || loud_fail "Full retained outer frame corners: $screen"
}
open_layout() {
    tmux send-keys -t "$session" C-o
    sleep 0.05
    tmux send-keys -t "$session" p
    wait_screen "h/j/k/l dock" "layout command did not open"
}

start_tui
wait_screen "Chat ▾" "Chat context did not render"
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
wait_screen "Chat ▾" "Chat context vanished in Full"
assert_full_chrome "Chat ▾"
if grep -aFq $'\033[<' "$ptydump".env.*.in.bin 2>/dev/null; then
    loud_fail "mouse drag leaked an SGR sequence into Chat PTY"
fi

# Keyboard-authoritative restore preserves the remembered 90% split. Then type
# through the same Chat pane; any leaked SGR prefix would corrupt this exact line.
open_layout
tmux send-keys -t "$session" l Enter
wait_layout mode split
wait_layout dock right
[[ $(layout_field size_percent) == 90 ]] || loud_fail "side restore lost exact remembered ratio"
tmux send-keys -t "$session" Tab
sleep 0.05
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
wait_screen "Task ▾" "Detail context did not render"
wait_screen "drag-detail-exact-id" "Detail identity changed"
open_layout
tmux send-keys -t "$session" j Enter
wait_layout dock bottom
wait_layout mode split
wait_screen "Task ▾" "stacked contextual seam did not render"
context_y=$(context_row_y "Task ▾") || loud_fail "could not locate stacked seam"
start_y=$((context_y - 1))
(( start_y >= 1 )) || loud_fail "invalid stacked seam coordinate $start_y"
sgr 0 60 "$start_y" M
sgr 32 60 1 M
sgr 32 60 4 M
sgr 0 60 4 m
wait_layout mode full
[[ $(layout_field dock) == bottom ]] || loud_fail "stacked drag changed desired dock"
[[ $(layout_field size_percent) == 90 ]] || loud_fail "stacked Full lost bounded split"
wait_screen "Task ▾" "Detail context vanished in Full"
assert_full_chrome "Task ▾"

# Kill only the outer TUI. Full is already atomically persisted; the inner Chat
# tmux process must survive and a fresh TUI must reload Full without respawn.
tmux kill-session -t "$session"
start_tui
wait_layout mode full
wait_screen "Chat ▾" "restarted TUI did not reload the full inspector"
[[ $(tmux display-message -p -t "$inner" '#{pane_pid}') == "$chat_pid" ]] || loud_fail "TUI restart respawned persistent Chat"
assert_full_chrome "Chat ▾"

# Restore Bottom exactly, then start a real drag and resize mid-gesture. The
# stale coordinates after SIGWINCH must be ignored rather than selecting,
# panning, forwarding to PTY, or snapping Full.
open_layout
tmux send-keys -t "$session" j Enter
wait_layout mode split
wait_layout dock bottom
[[ $(layout_field size_percent) == 90 ]] || loud_fail "restart restore lost remembered ratio"
wait_screen "Chat ▾" "stacked Chat context missing after restore"
context_y=$(context_row_y "Chat ▾") || loud_fail "could not locate restored stacked seam"
start_y=$((context_y - 1))
sgr 0 60 "$start_y" M
tmux resize-window -t "$session" -x 100 -y 28
sleep 0.15
sgr 32 50 1 M
sgr 0 50 1 m
sleep 0.15
[[ $(layout_field mode) == split ]] || loud_fail "stale post-resize drag snapped layout"
[[ $(layout_field dock) == bottom ]] || loud_fail "stale post-resize drag changed dock"
[[ $(layout_field size_percent) == 90 ]] || loud_fail "stale post-resize drag changed ratio"
if grep -aFq $'\033[<' "$ptydump".env.*.in.bin 2>/dev/null; then
    loud_fail "resize/drag leaked input to PTY"
fi
[[ $(tmux display-message -p -t "$inner" '#{pane_pid}') == "$chat_pid" ]] || loud_fail "resize respawned Chat"

echo "PASS: real side+stacked seam drags snap once to borderless Full; Chat/Detail restore, restart, resize, PTY identity and input confinement hold"
