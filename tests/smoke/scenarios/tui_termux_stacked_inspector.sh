#!/usr/bin/env bash
# Real tmux/SGR regression for responsive phone/Termux inspector stacking.
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
export HOME="$scratch/home" XDG_CONFIG_HOME="$scratch/home/.config" WG_GLOBAL_DIR="$scratch/home/.wg"
unset TMUX_TMPDIR
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$scratch/project"
G="$scratch/project/.wg"
"$WG_BIN" --dir "$G" init --no-agency >/dev/null
cat >"$G/config.toml" <<'TOML'
[dispatcher]
model = "claude:opus"
TOML
"$WG_BIN" --dir "$G" chat create --name termux-stack --command cat >/dev/null
"$WG_BIN" --dir "$G" add "termux-graph-anchor" -d "phone stack task detail" --no-place >/dev/null
cat >"$G/tui-state.json" <<'JSON'
{"layout":{"dock":"left","size_percent":63,"mode":"split"},"active_coordinator_id":0,"right_panel_tab":"Chat","open_tabs":[".chat-0"],"active":".chat-0"}
JSON
ptydump="$scratch/ptydump"
session="wg-termux-stack-$$"
inner=""
cleanup_session() {
  tmux kill-session -t "$session" 2>/dev/null || true
  [[ -z "$inner" ]] || tmux kill-session -t "$inner" 2>/dev/null || true
}
add_cleanup_hook cleanup_session
capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
wait_screen() {
  local needle=$1
  for _ in $(seq 1 200); do capture | grep -Fq "$needle" && return 0; sleep 0.03; done
  loud_fail "screen never contained '$needle': $(capture | tr '\n' '|')"
}
layout_field() {
  python3 - "$G/tui-state.json" "$1" <<'PY'
import json,sys
print(json.load(open(sys.argv[1],encoding='utf-8'))['layout'][sys.argv[2]])
PY
}
sgr() {
  local code=$1 x=$2 y=$3 suffix=$4
  tmux send-keys -t "$session" -l "$(printf '\033[<%s;%s;%s%s' "$code" "$x" "$y" "$suffix")"
}
assert_stacked() {
  local width=$1 screen context_y anchor_y marker_y
  screen=$(capture)
  context_y=$(python3 -c 'import sys
for y,row in enumerate(sys.stdin.read().splitlines(),1):
  if "↯  ⌁  ⌂  .chat-0" in row: print(y); break
' <<<"$screen")
  [[ -n "$context_y" ]] || loud_fail "$width: stacked context seam missing: $screen"
  anchor_y=$(python3 -c 'import sys
for y,row in enumerate(sys.stdin.read().splitlines(),1):
  if "termux-graph-anchor" in row: print(y); break
' <<<"$screen")
  marker_y=$(python3 -c 'import sys
ys=[y for y,row in enumerate(sys.stdin.read().splitlines(),1) if "PHONE_PTY_MARKER" in row]
print(max(ys) if ys else "")
' <<<"$screen")
  [[ -n "$anchor_y" && -n "$marker_y" ]] || loud_fail "$width: graph/Chat evidence missing: $screen"
  (( anchor_y < context_y && marker_y > context_y )) \
    || loud_fail "$width: inspector was not below graph (anchor=$anchor_y seam=$context_y marker=$marker_y): $screen"
  ! grep -q '│' <<<"$screen" || loud_fail "$width: stale Left/Right seam remained on phone: $screen"
  printf '%s\n' "$context_y"
}

# Start wide on an explicit advanced Left preference and prove the real side seam.
tmux new-session -d -s "$session" -x 140 -y 36 \
  "cd '$scratch/project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TERMUX_VERSION=0.119 WG_PTY_DUMP='$ptydump' '$WG_BIN' --dir '$G' tui"
tmux resize-window -t "$session" -x 140 -y 36
tmux set-option -t "$session" mouse on
wait_screen "↯  ⌁  ⌂  .chat-0"
for _ in $(seq 1 200); do
  inner=""
  while IFS= read -r candidate; do
    if tmux show-environment -t "$candidate" WG_DIR 2>/dev/null | grep -Fxq "WG_DIR=$G"; then
      inner=$candidate
      break
    fi
  done < <(tmux list-sessions -F '#S' 2>/dev/null | grep -E '^wg-chat-.*-chat-0$' || true)
  [[ -n "$inner" ]] && break
  sleep 0.03
done
[[ -n "$inner" ]] || loud_fail "inner command Chat session missing"
chat_pid=$(tmux display-message -p -t "$inner" '#{pane_pid}')
wide=$(capture)
grep -q '│' <<<"$wide" || loud_fail "wide explicit Left did not render a side seam: $wide"
[[ $(layout_field dock) == left ]] || loud_fail "wide desired Left was not loaded"

tmux send-keys -t "$session" Tab
sleep 0.05
tmux send-keys -t "$session" -l PHONE_PTY_MARKER
tmux send-keys -t "$session" Enter
wait_screen PHONE_PTY_MARKER

# Rotate to phone and Termux portrait widths: both panes remain usable and the
# desired Left/ratio stay persisted while presentation temporarily stacks.
for spec in '70 30' '40 22'; do
  set -- $spec
  tmux resize-window -t "$session" -x "$1" -y "$2"
  sleep 0.2
  context_y=$(assert_stacked "$1")
  pty_w=0 pty_h=0
  for _ in $(seq 1 100); do
    read -r pty_w pty_h <<<"$(tmux display-message -p -t "$inner" '#{pane_width} #{pane_height}')"
    [[ "$pty_w" == "$1" && "$pty_h" -ge 3 ]] && break
    sleep 0.03
  done
  [[ "$pty_w" == "$1" && "$pty_h" -ge 3 ]] \
    || loud_fail "$1: Chat child did not receive usable full-width stacked PTY dimensions ($pty_w x $pty_h)"
  [[ $(layout_field dock) == left ]] || loud_fail "$1: responsive fallback persisted Bottom over Left"
done

# On the live 40-column frame, the horizontal seam owns the mouse gesture.
# Drag down two rows (vertical axis), release, and prove ratio changed without
# click fall-through or rewriting the desired side.
before=$(layout_field size_percent)
start_y=$((context_y - 1))
end_y=$((start_y + 2))
sgr 0 20 "$start_y" M
sgr 32 20 "$end_y" M
sgr 0 20 "$end_y" m
for _ in $(seq 1 120); do
  after=$(layout_field size_percent 2>/dev/null || echo "$before")
  [[ "$after" != "$before" ]] && break
  sleep 0.03
done
[[ "$after" != "$before" ]] || loud_fail "phone horizontal seam did not resize on vertical drag"
[[ $(layout_field dock) == left ]] || loud_fail "phone seam drag persisted effective Bottom"
[[ $(layout_field mode) == split ]] || loud_fail "short seam drag unexpectedly left Split"

# Direct Task and Chat controls remain usable in the stack.
tmux send-keys -t "$session" 1
wait_screen termux-graph-anchor
tmux send-keys -t "$session" 0
wait_screen PHONE_PTY_MARKER

# Rotate back wide: exact Left and dragged ratio restore, Chat PTY identity and
# input stream survive, and no SGR mouse bytes were stolen by the child.
tmux resize-window -t "$session" -x 140 -y 36
sleep 0.25
restored=$(capture)
grep -q '│' <<<"$restored" || loud_fail "wide rotation did not restore Left/Right seam: $restored"
[[ $(layout_field dock) == left && $(layout_field size_percent) == "$after" ]] \
  || loud_fail "wide rotation lost exact desired Left/ratio"
[[ $(tmux display-message -p -t "$inner" '#{pane_pid}') == "$chat_pid" ]] \
  || loud_fail "phone rotation respawned Chat PTY"
read -r restored_pty_w restored_pty_h <<<"$(tmux display-message -p -t "$inner" '#{pane_width} #{pane_height}')"
(( restored_pty_w > 20 && restored_pty_w < 140 && restored_pty_h >= 3 )) \
  || loud_fail "wide side restore delivered unusable Chat PTY dimensions ($restored_pty_w x $restored_pty_h)"
tmux send-keys -t "$session" -l AFTER_PHONE_ROTATION
tmux send-keys -t "$session" Enter
wait_screen AFTER_PHONE_ROTATION
python3 - "$ptydump" <<'PY'
import glob,sys
payload=b''.join(open(p,'rb').read() for p in glob.glob(sys.argv[1]+'.env.*.in.bin'))
assert b'PHONE_PTY_MARKER\r' in payload, payload
assert b'AFTER_PHONE_ROTATION\r' in payload, payload
assert b'\x1b[<' not in payload, payload
PY

echo "PASS: wide Left restored around live 70/40-column stacked phone inspector; vertical seam, Chat/Task, PTY identity/input, and desired ratio remained correct"
