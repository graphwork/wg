#!/usr/bin/env bash
# One-row contextual, borderless inspector human flow. Drives the real TUI
# through tmux at desktop, medium, and Termux sizes with mouse disabled.
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
export TMUX_TMPDIR="$scratch/tmux"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$TMUX_TMPDIR" "$scratch/project"
G="$scratch/project/.wg"
"$WG_BIN" --dir "$G" init --no-agency >/dev/null
"$WG_BIN" --dir "$G" add "layout-fixture-exact-id" -d "render both panes" --no-place >/dev/null
: >"$G/config.toml"
graph_before=$(sha256sum "$G/graph.jsonl" | cut -d' ' -f1)
config_before=$(sha256sum "$G/config.toml" | cut -d' ' -f1)

session="wg-one-row-layout-$$"
add_cleanup_hook "tmux kill-session -t '$session' 2>/dev/null || true"
capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
wait_screen() {
  local needle=$1
  for _ in $(seq 1 160); do
    capture | grep -qF "$needle" && return 0
    sleep 0.05
  done
  loud_fail "screen never contained '$needle': $(capture | tail -25)"
}
layout_field() {
  python3 - "$G/tui-state.json" "$1" <<'PY'
import json, sys
print(str(json.load(open(sys.argv[1], encoding="utf-8"))["layout"][sys.argv[2]]).lower())
PY
}
wait_layout() {
  local field=$1 expected=$2
  for _ in $(seq 1 120); do
    [[ $(layout_field "$field" 2>/dev/null || true) == "$expected" ]] && return 0
    sleep 0.05
  done
  loud_fail "layout $field did not become $expected"
}
open_layout() {
  tmux send-keys -t "$session" p
  wait_screen "h/j/k/l dock"
}
assert_one_context_row() {
  local needle=$1 screen count
  screen=$(capture)
  count=$(grep -oF "$needle" <<<"$screen" | wc -l | tr -d ' ')
  [[ "$count" == 1 ]] || loud_fail "expected exactly one '$needle' context row, got $count: $screen"
  ! grep -qF "Graph | NAV" <<<"$screen" || loud_fail "legacy global status row remains"
  ! grep -qF "Commands •" <<<"$screen" || loud_fail "legacy WG footer remains"
}

# Start in Chat without creating anything, then switch to the contextual Task
# inspector for the split captures.
tmux new-session -d -s "$session" -x 160 -y 44 \
  "cd '$scratch/project' && env TERMUX_VERSION=0.119 MOSH_CONNECTION='smoke 0 0' MOSH_SERVER_PID=4242 '$WG_BIN' --dir '$G' tui --no-mouse"
wait_screen "[ New chat ]"
assert_one_context_row "Chat ▾"
capture >"$scratch/wide-chat-side.txt"
tmux send-keys -t "$session" 1
wait_screen "Task ▾"
# Inspect the real graph row rather than accepting a generic "no task" shell.
tmux send-keys -t "$session" Down Enter
wait_screen "Task ▾  layout-fixture-exact  ● open"
assert_one_context_row "Task ▾"
capture >"$scratch/wide-task-stacked-initial.txt"
open_layout; tmux send-keys -t "$session" l Enter
wait_layout dock right
wait_layout mode split
wait_screen "Task ▾  layout-fixture-exact  ● open"
assert_one_context_row "Task ▾"
capture >"$scratch/wide-task-side.txt"

# Stacked split: the Task context is embedded into the one horizontal seam.
open_layout; tmux send-keys -t "$session" j Enter
wait_layout dock bottom
wait_screen "Task ▾"
assert_one_context_row "Task ▾"
capture >"$scratch/wide-task-stacked.txt"

# Full inspector has no outer restore frame and still exactly one context row.
open_layout; tmux send-keys -t "$session" f Enter
wait_layout mode full
wait_screen "Task ▾"
assert_one_context_row "Task ▾"
capture >"$scratch/wide-task-full.txt"

# Chat exposes only contextual chat controls and a fully-labelled fixed action.
tmux send-keys -t "$session" 0
wait_screen "[ New chat ]"
assert_one_context_row "Chat ▾"
chat_screen=$(capture)
grep -qF "[ New chat ]" <<<"$chat_screen" || loud_fail "fully-labelled New chat missing"
! grep -qF "Task ▾" <<<"$chat_screen" || loud_fail "task controls leaked into Chat context"
capture >"$scratch/wide-chat-full.txt"

# Medium and Termux widths keep the labelled action. Optional route/actions
# collapse rather than creating a second row.
for spec in "76 30 medium" "40 22 termux"; do
  set -- $spec
  # Exercise a resize burst before settling at each measured viewport.
  tmux resize-window -t "$session" -x 58 -y 24
  tmux resize-window -t "$session" -x 104 -y 32
  tmux resize-window -t "$session" -x "$1" -y "$2"
  sleep 0.25
  wait_screen "[ New chat ]"
  assert_one_context_row "Chat ▾"
  capture >"$scratch/$3-chat.txt"
done

# Keyboard-authoritative live preview and rollback remain available with mouse
# disabled. Layout mode replaces the same context row.
open_layout
layout_screen=$(capture)
[[ $(grep -oF "Layout" <<<"$layout_screen" | wc -l | tr -d ' ') == 1 ]] \
  || loud_fail "Layout mode added rows: $layout_screen"
tmux send-keys -t "$session" h '+' Escape
sleep 0.15
# Re-open and commit; narrow→wide restoration must retain desired state.
open_layout; tmux send-keys -t "$session" h '=' Enter
wait_layout dock left
wait_layout mode split
tmux resize-window -t "$session" -x 160 -y 44
sleep 0.25
wait_screen "Chat"
[[ $(layout_field dock) == left ]] || loud_fail "wide restoration lost desired dock"

[[ $(sha256sum "$G/graph.jsonl" | cut -d' ' -f1) == "$graph_before" ]] \
  || loud_fail "TUI layout mutated graph"
[[ $(sha256sum "$G/config.toml" | cut -d' ' -f1) == "$config_before" ]] \
  || loud_fail "TUI layout mutated config"

echo "PASS: one contextual row, exact Task context, fixed New chat, side/stacked/full seam rules, resize bursts, keyboard layout mode, and responsive restoration"
