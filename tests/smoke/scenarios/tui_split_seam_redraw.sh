#!/usr/bin/env bash
# Real tmux/back-buffer regression for the single Graph/inspector seam. The
# human flow moves the live seam by arbitrary keyboard sizing and a named
# preset at desktop and Termux-phone geometry, then crosses Full/Hidden and a
# responsive rotation. Captures are checked at physical cell coordinates.
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
unset TMUX WG_DIR WG_TASK_ID WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER
export TMUX_TMPDIR="$scratch/tmux"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$TMUX_TMPDIR" "$scratch/project"
G="$scratch/project/.wg"
"$WG_BIN" --dir "$G" init --no-agency >/dev/null
"$WG_BIN" --dir "$G" add "Archived seam chat" --id .chat-0 -t chat-loop -t archived --no-place >/dev/null 2>&1
# Long independent rows put real Graph dashes/text at every prospective desktop
# seam column; enough rows do the same at both prospective phone seam rows.
for n in $(seq -w 1 24); do
  "$WG_BIN" --dir "$G" add \
    "Graph seam seed $n abcdefghijklmnopqrstuvwxyz 0123456789 abcdefghijklmnopqrstuvwxyz" \
    --id "seam-seed-$n-abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789" \
    -d "Graph cells must be replaced, never recolored, when the split moves." \
    --no-place >/dev/null 2>&1
done
cat >"$G/tui-state.json" <<'JSON'
{"layout":{"dock":"right","size_percent":33,"mode":"split"},"active_coordinator_id":0,"right_panel_tab":"Detail","open_tabs":[".chat-0"],"active":".chat-0"}
JSON

session="wg-split-seam-redraw-$$"
cleanup_session() { tmux kill-session -t "$session" 2>/dev/null || true; }
add_cleanup_hook cleanup_session
capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
snapshot() { capture >"$1"; }
wait_screen() {
  local needle=$1 label=${2:-"screen missing $1"}
  for _ in $(seq 1 240); do capture | grep -Fq "$needle" && return 0; sleep 0.03; done
  loud_fail "$label: $(capture | tr '\n' '|')"
}
layout_field() {
  python3 - "$G/tui-state.json" "$1" <<'PY'
import json,sys
try: print(str(json.load(open(sys.argv[1], encoding="utf-8"))["layout"][sys.argv[2]]).lower())
except Exception: pass
PY
}
wait_layout() {
  local field=$1 expected=$2
  for _ in $(seq 1 240); do
    [[ $(layout_field "$field") == "$expected" ]] && return 0
    sleep 0.03
  done
  loud_fail "layout $field did not become $expected"
}
open_layout() {
  tmux send-keys -t "$session" p
  for _ in $(seq 1 240); do
    capture | grep -Eq 'Size:|[[:space:]]P:(Split|Full|Hidden)' && return 0
    sleep 0.03
  done
  loud_fail "Panel/Layout row did not open: $(capture | tr '\n' '|')"
}
assert_nonblank_burst() {
  local flag=$1
  rm -f "$flag"
  for _ in $(seq 1 35); do
    if [[ -z "$(capture | tr -d '[:space:]')" ]]; then
      : >"$flag"
      return
    fi
    sleep 0.008
  done
}
assert_no_global_clear() {
  local raw=$1 label=$2
  python3 - "$raw" "$label" <<'PY'
import sys
raw=open(sys.argv[1],"rb").read()
for seq in (b"\x1b[2J", b"\x1b[3J", b"\x1b[H\x1b[2J"):
    if seq in raw:
        raise SystemExit(f"{sys.argv[2]} used a global terminal clear: {seq!r}")
PY
}
assert_vertical_move() {
  local before=$1 after=$2 old_x=$3 new_x=$4 width=$5 height=$6 label=$7
  python3 - "$before" "$after" "$old_x" "$new_x" "$width" "$height" "$label" <<'PY'
import sys
bp,ap,old_x,new_x,width,height,label=sys.argv[1:]
old_x,new_x,width,height=map(int,(old_x,new_x,width,height))
def rows(path):
    data=open(path,encoding="utf-8",errors="replace").read().splitlines()
    data=(data+[""]*height)[:height]
    return [(r+" "*width)[:width] for r in data]
before,after=rows(bp),rows(ap)
seed=sum(before[y][new_x] not in " │" for y in range(height))
assert seed >= 2, f"{label}: prospective seam did not cover seeded Graph glyphs ({seed})"
wrong=[(y,after[y][new_x]) for y in range(height) if after[y][new_x] != "│"]
assert not wrong, f"{label}: current seam not fully glyph-painted: {wrong[:8]}"
ghost=[y for y in range(1,height) if after[y][old_x] == "│"]
assert not ghost, f"{label}: retired seam glyphs remain at rows {ghost}"
full=[x for x in range(width) if all(after[y][x] == "│" for y in range(height))]
assert full == [new_x], f"{label}: expected one vertical seam at {new_x}, got {full}"
assert sum(bool(r.strip()) for r in after) >= 5, f"{label}: blank/flickered final frame"
assert not any(c in "┌┐└┘" for r in after for c in r), f"{label}: outer frame appeared"
PY
}
assert_horizontal_move() {
  local before=$1 after=$2 old_y=$3 new_y=$4 width=$5 height=$6 label=$7
  python3 - "$before" "$after" "$old_y" "$new_y" "$width" "$height" "$label" <<'PY'
import sys
bp,ap,old_y,new_y,width,height,label=sys.argv[1:]
old_y,new_y,width,height=map(int,(old_y,new_y,width,height))
def rows(path):
    data=open(path,encoding="utf-8",errors="replace").read().splitlines()
    data=(data+[""]*height)[:height]
    return [(r+" "*width)[:width] for r in data]
before,after=rows(bp),rows(ap)
assert before[new_y].strip(), f"{label}: prospective seam row had no seeded Graph text"
context="↯  ⌁  ⌂"
assert context not in before[new_y], f"{label}: prospective seam was not a Graph row"
assert context in after[new_y], f"{label}: current contextual seam was not fully repainted: {after[new_y]!r}"
assert context not in after[old_y], f"{label}: retired contextual seam remains: {after[old_y]!r}"
assert after[old_y].count("─") < width//2, f"{label}: retired dash seam remains: {after[old_y]!r}"
rows_with_context=[y for y,r in enumerate(after) if context in r]
assert rows_with_context == [new_y], f"{label}: expected one contextual seam, got {rows_with_context}"
assert sum(bool(r.strip()) for r in after) >= 5, f"{label}: blank/flickered final frame"
assert not any(c in "┌┐└┘" for r in after for c in r), f"{label}: outer frame appeared"
PY
}

# Run through real tmux with the same outer-transport hints used by Termux +
# mosh. Desktop begins as a side split at an arbitrary persisted 33%.
tmux new-session -d -s "$session" -x 120 -y 30 \
  "cd '$scratch/project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TERMUX_VERSION=0.119 MOSH_CONNECTION='smoke 0 0' MOSH_SERVER_PID=4242 '$WG_BIN' --dir '$G' tui --no-mouse 2>'$scratch/tui.err'"
tmux set-option -t "$session" remain-on-exit on
tmux resize-window -t "$session" -x 120 -y 30
wait_screen "seam-seed-" "seeded Graph did not render"
# Startup is deliberately storage-independent; an asynchronous Chat startup
# apply may land after the first graph shell. Set the measured baseline through
# the real visible editor only after that generation has settled.
sleep 0.4
open_layout
tmux send-keys -t "$session" l 1 Enter
wait_layout dock right
wait_layout size_percent 33
wait_layout mode split
wait_screen "seam-seed-"
sleep 0.1
snapshot "$scratch/desktop-33.txt"

# Capture only the transition bytes: ordinary percentage changes must use the
# Ratatui diff, never a full terminal clear/blank frame.
raw_desktop="$scratch/desktop-transition.raw"
: >"$raw_desktop"
tmux pipe-pane -t "$session" "cat >'$raw_desktop'"
assert_nonblank_burst "$scratch/desktop-blank" & monitor=$!
open_layout
tmux send-keys -t "$session" '+' Enter
wait_layout size_percent 38
wait "$monitor"
[[ ! -e "$scratch/desktop-blank" ]] || loud_fail "desktop arbitrary resize emitted a blank frame"
snapshot "$scratch/desktop-38.txt"
# Right dock: width=120, 33% => old seam x=80; 38% => new seam x=74.
assert_vertical_move "$scratch/desktop-33.txt" "$scratch/desktop-38.txt" 80 74 120 30 "desktop 33→38"

assert_nonblank_burst "$scratch/preset-blank" & monitor=$!
open_layout
tmux send-keys -t "$session" 3 Enter
wait_layout size_percent 67
wait "$monitor"
[[ ! -e "$scratch/preset-blank" ]] || loud_fail "desktop preset emitted a blank frame"
snapshot "$scratch/desktop-67.txt"
# 38% => old seam x=74; 67% => new seam x=39.
assert_vertical_move "$scratch/desktop-38.txt" "$scratch/desktop-67.txt" 74 39 120 30 "desktop 38→67 preset"
tmux pipe-pane -t "$session"
assert_no_global_clear "$raw_desktop" "desktop seam moves"

# Full and Hidden remove the seam rather than leaving an old full-height line;
# Split restoration deterministically repaints the exact remembered seam.
open_layout; tmux send-keys -t "$session" f Enter
wait_layout mode full
snapshot "$scratch/desktop-full.txt"
python3 - "$scratch/desktop-full.txt" <<'PY'
import sys
rows=open(sys.argv[1],encoding="utf-8",errors="replace").read().splitlines()
assert not any("│" in r for r in rows), "Full retained a split seam or outer frame"
assert sum("↯  ⌁  ⌂" in r for r in rows) == 1, "Full lost/duplicated contextual row"
PY
open_layout; tmux send-keys -t "$session" l Enter
wait_layout mode split
snapshot "$scratch/desktop-restored.txt"
python3 - "$scratch/desktop-restored.txt" <<'PY'
import sys
rows=(open(sys.argv[1],encoding="utf-8",errors="replace").read().splitlines()+[""]*30)[:30]
rows=[(r+" "*120)[:120] for r in rows]
assert all(r[39] == "│" for r in rows), "Full restore did not repaint the complete remembered seam"
PY
open_layout; tmux send-keys -t "$session" 0 Enter
wait_layout mode hidden
snapshot "$scratch/desktop-hidden.txt"
python3 - "$scratch/desktop-hidden.txt" <<'PY'
import sys
rows=open(sys.argv[1],encoding="utf-8",errors="replace").read().splitlines()
assert not any("│" in r for r in rows), "Hidden retained the old split seam"
assert sum("↯  ⌁  ⌂" in r for r in rows) == 1, "Hidden must retain one contextual row"
PY
open_layout; tmux send-keys -t "$session" l 1 Enter
wait_layout mode split
wait_layout size_percent 33
wait_layout dock right

# Rotate to a phone viewport. Right resolves responsively to Bottom while the
# desired dock remains Right. Capture a real Graph row that will become the new
# horizontal contextual seam, then move it by the named 2/3 preset.
tmux resize-window -t "$session" -x 70 -y 31
for _ in $(seq 1 120); do
  dims=$(tmux display-message -p -t "$session" '#{pane_width}x#{pane_height}')
  [[ "$dims" == 70x31 ]] && break
  sleep 0.03
done
wait_screen "seam-seed-" "phone Graph did not render"
sleep 0.15
snapshot "$scratch/phone-33.txt"
raw_phone="$scratch/phone-transition.raw"
: >"$raw_phone"
tmux pipe-pane -t "$session" "cat >'$raw_phone'"
assert_nonblank_burst "$scratch/phone-blank" & monitor=$!
open_layout
tmux send-keys -t "$session" 3 Enter
wait_layout size_percent 67
wait "$monitor"
[[ ! -e "$scratch/phone-blank" ]] || loud_fail "phone preset emitted a blank frame"
snapshot "$scratch/phone-67.txt"
tmux pipe-pane -t "$session"
# height=31: 33% => graph 20 + seam row 20; 67% => graph 10 + seam row 10.
assert_horizontal_move "$scratch/phone-33.txt" "$scratch/phone-67.txt" 20 10 70 31 "phone 33→67"
assert_no_global_clear "$raw_phone" "phone seam move"

# Rotate back: Auto/responsive history must not leave the phone row or double
# the restored side seam.
tmux resize-window -t "$session" -x 120 -y 30
wait_screen "seam-seed-" "wide Graph did not return after rotation"
sleep 0.15
snapshot "$scratch/rotated-wide.txt"
python3 - "$scratch/rotated-wide.txt" <<'PY'
import sys
rows=(open(sys.argv[1],encoding="utf-8",errors="replace").read().splitlines()+[""]*30)[:30]
rows=[(r+" "*120)[:120] for r in rows]
full=[x for x in range(120) if all(rows[y][x] == "│" for y in range(30))]
assert full == [39], f"wide rotation restored a ghost/doubled seam: {full}"
assert sum("↯  ⌁  ⌂" in r for r in rows) == 1, "rotation duplicated contextual row"
PY

echo "PASS: isolated tmux desktop+phone buffers show one fully glyph-painted seam, cleared retired cells, no doubled/ghost line, no percentage-move global clear or captured blank frame, and clean Full/Hidden/rotation transitions"
