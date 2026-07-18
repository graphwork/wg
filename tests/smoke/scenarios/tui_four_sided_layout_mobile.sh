#!/usr/bin/env bash
# Four-sided inspector human flow. Drives the real TUI through tmux PTYs:
# keyboard-only Termux+mosh command mode, phone rotation/restoration, and all
# four SGR mouse edge drags (plus SIGWINCH cancellation mid-adjustment).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "cargo is required to build the unmerged candidate"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required for the real TUI flow"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required to inspect atomic TUI state"

REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
cd "$REPO_ROOT"
cargo build --quiet --bin wg
WG_BIN="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg"
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"

scratch=$(make_scratch)
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
export TMUX_TMPDIR="$scratch/tmux"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$TMUX_TMPDIR"
G="$scratch/project/.wg"
mkdir -p "$scratch/project"
"$WG_BIN" --dir "$G" init --no-agency >"$scratch/init.log" 2>&1 \
  || loud_fail "wg init failed: $(tail -30 "$scratch/init.log")"
"$WG_BIN" --dir "$G" add "layout fixture" -d "render both panes" --no-place >"$scratch/add.log" 2>&1 \
  || loud_fail "fixture add failed: $(tail -30 "$scratch/add.log")"
# Graph-only init intentionally need not create provider config. An empty local
# file is a byte-stable sentinel proving Layout never writes routing state.
: >"$G/config.toml"

graph_before=$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)
config_before=$(sha256sum "$G/config.toml" | cut -d' ' -f1)
keyboard="wg-layout-keyboard-$$"
mouse="wg-layout-mouse-$$"
cleanup_sessions() {
  tmux kill-session -t "$keyboard" 2>/dev/null || true
  tmux kill-session -t "$mouse" 2>/dev/null || true
}
add_cleanup_hook cleanup_sessions

capture() { tmux capture-pane -p -t "$1" 2>/dev/null || true; }
wait_screen() {
  local session=$1 needle=$2
  for _ in $(seq 1 120); do
    capture "$session" | grep -qF "$needle" && return 0
    sleep 0.05
  done
  return 1
}
layout_field() {
  local field=$1
  python3 - "$G/tui-state.json" "$field" <<'PY'
import json, sys
try:
    value=json.load(open(sys.argv[1], encoding="utf-8"))["layout"][sys.argv[2]]
except Exception:
    raise SystemExit(1)
print(str(value).lower())
PY
}
wait_layout() {
  local field=$1 expected=$2
  for _ in $(seq 1 120); do
    value=$(layout_field "$field" 2>/dev/null || true)
    [[ "$value" == "$expected" ]] && return 0
    sleep 0.05
  done
  loud_fail "layout $field never became $expected (actual=$(layout_field "$field" 2>/dev/null || echo missing)); screen=$(capture "${3:-$keyboard}" | tail -20 | tr '\n' ' '); trace=$(tail -12 "${mouse_trace:-/dev/null}" 2>/dev/null | tr '\n' ' ')"
}
open_layout() {
  local session=$1
  tmux send-keys -t "$session" p
  wait_screen "$session" "Layout — keyboard works" \
    || loud_fail "plain p did not open visible Layout overlay: $(capture "$session" | tail -25)"
}

# Mouse disabled, ordinary keys only, under the conservative mosh path and a
# Termux-shaped environment. This is the authoritative mobile workflow.
tmux new-session -d -s "$keyboard" -x 160 -y 44 \
  "cd '$scratch/project' && env TERMUX_VERSION=0.119 MOSH_CONNECTION='smoke 0 0' MOSH_SERVER_PID=4242 '$WG_BIN' --dir '$G' tui --no-mouse"
wait_screen "$keyboard" "Graph | NAV" \
  || loud_fail "keyboard TUI never rendered: $(capture "$keyboard" | tail -25)"

open_layout "$keyboard"
# Initial desired size is 67: '=' wraps to 33. The highlighted dock/cheat line
# must update live; Enter atomically commits it.
tmux send-keys -t "$keyboard" h '=' Enter
wait_layout dock left
wait_layout size_percent 33
wait_layout mode split

# Direct four-way keyboard docking (vim directions), Auto, grow/shrink,
# Full/Hide, and Esc rollback. No mouse or modifier-dependent key is required.
for spec in 'j bottom' 'k top' 'l right' 'a auto'; do
  key=${spec%% *}; expected=${spec##* }
  open_layout "$keyboard"
  tmux send-keys -t "$keyboard" "$key" Enter
  wait_layout dock "$expected"
done
open_layout "$keyboard"
tmux send-keys -t "$keyboard" '+' '+' '-'
tmux send-keys -t "$keyboard" Enter
wait_layout size_percent 38
open_layout "$keyboard"; tmux send-keys -t "$keyboard" f Enter; wait_layout mode full
open_layout "$keyboard"; tmux send-keys -t "$keyboard" 0 Enter; wait_layout mode hidden
before_cancel=$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["layout"],sort_keys=True))' "$G/tui-state.json")
open_layout "$keyboard"; tmux send-keys -t "$keyboard" h '+' Escape
sleep 0.15
after_cancel=$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["layout"],sort_keys=True))' "$G/tui-state.json")
[[ "$after_cancel" == "$before_cancel" ]] || loud_fail "Esc failed to restore the pre-modal snapshot"

# Set a sticky explicit preference, rotate narrow→compact→wide while the modal
# is active once, then commit. Desired state must survive every transient size.
open_layout "$keyboard"
tmux send-keys -t "$keyboard" h '-' '+' '+' '+' '+' '+' '+' # 38 -> 33 -> 63
tmux resize-window -t "$keyboard" -x 40 -y 22
sleep 0.12
tmux resize-window -t "$keyboard" -x 76 -y 30
sleep 0.12
tmux resize-window -t "$keyboard" -x 160 -y 44
tmux send-keys -t "$keyboard" Enter
wait_layout dock left
wait_layout size_percent 63
[[ $(sha256sum "$G/graph.jsonl" | cut -d' ' -f1) == "$graph_before" ]] \
  || loud_fail "layout preference mutated graph/chats"
[[ $(sha256sum "$G/config.toml" | cut -d' ' -f1) == "$config_before" ]] \
  || loud_fail "layout preference mutated provider config"
tmux kill-session -t "$keyboard"

# Pointer convenience: exercise real SGR 1006 reports at each fullscreen edge.
# tmux's outer pane is the PTY; send-keys -l injects exactly what a desktop
# terminal sends after wg enables mouse capture.
mouse_trace="$scratch/mouse-trace.jsonl"
tmux new-session -d -s "$mouse" -x 160 -y 44 \
  "cd '$scratch/project' && env TERMUX_VERSION=0.119 '$WG_BIN' --dir '$G' tui --trace '$mouse_trace'"
wait_screen "$mouse" "Graph | NAV" \
  || loud_fail "mouse TUI never rendered: $(capture "$mouse" | tail -25)"
tmux resize-window -t "$mouse" -x 160 -y 44
# Let SIGWINCH settle and async startup apply its persisted desired layout.
sleep 0.35
[[ $(tmux display-message -p -t "$mouse" '#{window_width}x#{window_height}') == 160x44 ]] \
  || loud_fail "mouse PTY did not resize to 160x44 (got $(tmux display-message -p -t "$mouse" '#{window_width}x#{window_height}'))"
send_sgr() { tmux send-keys -t "$mouse" -l "$1"; sleep 0.04; }
set_full() {
  open_layout "$mouse"
  tmux send-keys -t "$mouse" f Enter
  wait_layout mode full "$mouse"
  sleep 0.08
}
drag_edge() {
  local expected=$1 press=$2 drag=$3 release=$4
  set_full
  send_sgr "$press"; send_sgr "$drag"; send_sgr "$release"
  wait_layout dock "$expected" "$mouse"
  wait_layout mode split "$mouse"
  pct=$(layout_field size_percent)
  (( pct >= 10 && pct <= 90 )) || loud_fail "edge drag produced unbounded percent $pct"
}
# SGR coordinates are 1-based. Main viewport is y=2..42 at 160x44.
drag_edge right  $'\e[<0;1;20M'   $'\e[<32;25;20M'  $'\e[<0;25;20m'
drag_edge left   $'\e[<0;160;20M' $'\e[<32;136;20M' $'\e[<0;136;20m'
drag_edge bottom $'\e[<0;80;2M'   $'\e[<32;80;12M'  $'\e[<0;80;12m'
drag_edge top    $'\e[<0;80;42M'  $'\e[<32;80;30M'  $'\e[<0;80;30m'

# Resize between pointer-down and motion. The stale reports must be ignored and
# the pointer-down Full preference restored, never inverted or jumped.
set_full
full_before=$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["layout"],sort_keys=True))' "$G/tui-state.json")
send_sgr $'\e[<0;1;20M'
tmux resize-window -t "$mouse" -x 120 -y 34
sleep 0.12
send_sgr $'\e[<32;80;20M'; send_sgr $'\e[<0;80;20m'
sleep 0.15
full_after=$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["layout"],sort_keys=True))' "$G/tui-state.json")
[[ "$full_after" == "$full_before" ]] \
  || loud_fail "resize during active edge drag changed desired state: before=$full_before after=$full_after"

echo "PASS: keyboard-only mobile layout, compact rotation restore, all four desktop edge drags, and resize cancellation passed without graph/config mutation"
