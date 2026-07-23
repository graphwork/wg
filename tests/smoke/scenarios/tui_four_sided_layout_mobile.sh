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
"$WG_BIN" --dir "$G" add "Archived chat" --id .chat-0 -t chat-loop -t archived --no-place >/dev/null
"$WG_BIN" --dir "$G" add "layout-fixture-exact-id" -d "render both panes" --no-place >/dev/null
: >"$G/config.toml"
# Daemon-produced disk cache fixture: the TUI may only read this through its
# asynchronous cache, never by doing filesystem work in render/input.
mkdir -p "$G/service/disk"
cat >"$G/service/disk/disk-sentinel.json" <<'JSON'
{"schema":1,"generated_at":"2026-07-20T00:00:00Z","level":"healthy","reason":"smoke","mounts":[],"targets":[],"worktrees":{"path":"worktrees","bytes":0,"complete":true},"agents":{"path":"agents","bytes":0,"complete":true},"log":{"path":"log","bytes":0,"complete":true},"active_builds":0,"active_build_heavy":0,"projected_headroom_bytes":45097156608}
JSON
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
  loud_fail "screen never contained '$needle': pane=$(tmux list-panes -t "$session" -F '#{pane_dead}:#{pane_dead_status}:#{pane_current_command}:#{pane_width}x#{pane_height}' 2>/dev/null || true) screen=$(tmux capture-pane -ep -S -100 -t "$session" 2>/dev/null | tail -40) stderr=$(cat "$scratch/tui.err" 2>/dev/null || true)"
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
wait_layout_row() {
  for _ in $(seq 1 160); do
    capture | grep -Eq 'Dock:.*Size:.*Pending:|[[:space:]]P:(Split|Full|Hidden)' && return 0
    sleep 0.05
  done
  loud_fail "discoverable layout row never appeared: $(capture | tail -25)"
}
open_layout() {
  tmux send-keys -t "$session" p
  wait_layout_row
}
assert_one_context_row() {
  local screen count
  screen=$(capture)
  # Full inserts its visible ↕/↔ Split escape between the three destinations
  # and the current identity, so count the invariant lane prefix itself.
  count=$(grep -oF "↯  ⌁  ⌂" <<<"$screen" | wc -l | tr -d ' ')
  [[ "$count" == 1 ]] || loud_fail "expected exactly one contextual row, got $count: $screen"
  ! grep -qF "Graph | NAV" <<<"$screen" || loud_fail "legacy global status row remains"
  ! grep -qF "Commands •" <<<"$screen" || loud_fail "legacy WG footer remains"
}

# Start with every chat archived. Recovery must still expose the global action
# without implicitly creating/resurrecting a chat, then switch to Task.
tmux new-session -d -s "$session" -x 160 -y 44 \
  "cd '$scratch/project' && env TERMUX_VERSION=0.119 MOSH_CONNECTION='smoke 0 0' MOSH_SERVER_PID=4242 '$WG_BIN' --dir '$G' tui --no-mouse 2>'$scratch/tui.err'"
tmux set-option -t "$session" remain-on-exit on
chat_context="↯  ⌁  ⌂"
task_context="↯  ⌁  ⌂  layout-fixture-exact"
wait_screen "[ New chat ]"
wait_screen "$chat_context"
assert_one_context_row "$chat_context"
capture >"$scratch/wide-chat-side.txt"
tmux send-keys -t "$session" 1
# Inspect the real graph row rather than accepting a generic "no task" shell.
# All chats are archived, so explicitly return focus to the graph first.
tmux send-keys -t "$session" Tab Down Enter
wait_screen "$task_context"
assert_one_context_row "$task_context"
capture >"$scratch/wide-task-stacked-initial.txt"
open_layout
first_use=$(capture)
grep -Eq 'Dock:Auto.*Size:67%.*Pending:Split|Auto 67% P:Split' <<<"$first_use" \
  || loud_fail "first-use row did not identify current dock/percentage/pending mode: $first_use"
for label in 'h:Left' 'j:Bottom' 'k:Top' 'l:Right' 'Apply' 'Esc'; do
  grep -qF "$label" <<<"$first_use" || loud_fail "first-use row did not explain '$label': $first_use"
done
# Named presets remain an explicit compact page rather than a cryptic cycle.
tmux send-keys -t "$session" p
# At wide widths the full Dock/Size/Pending summary remains stable while the
# visible s:Size return control proves the Presets page owns this frame.
wait_screen 's:Size'
preset_page=$(capture)
for label in '1:1/3' '2:1/2' '3:2/3'; do
  grep -qF "$label" <<<"$preset_page" || loud_fail "preset page lost '$label': $preset_page"
done
tmux send-keys -t "$session" s
wait_layout_row
# Dock and sizing are separate operations: l selects Right; +/- move the live
# arbitrary percentage by exactly one 5% step before Enter persists it.
tmux send-keys -t "$session" l '+'
wait_screen "72%"
tmux send-keys -t "$session" '-'
wait_screen "67%"
tmux send-keys -t "$session" Enter
wait_layout dock right
wait_layout size_percent 67
wait_layout mode split
wait_screen "$task_context"
assert_one_context_row "$task_context"
capture >"$scratch/wide-task-side.txt"

# Stacked split: the Task context is embedded into the one horizontal seam.
open_layout; tmux send-keys -t "$session" j Enter
wait_layout dock bottom
wait_screen "$task_context"
assert_one_context_row "$task_context"
capture >"$scratch/wide-task-stacked.txt"

# Full inspector has no outer restore frame and still exactly one context row.
open_layout; tmux send-keys -t "$session" f Enter
wait_layout mode full
wait_screen "layout-fixture-exact"
assert_one_context_row "$task_context"
task_full=$(capture)
grep -qF "[ New chat ]" <<<"$task_full" || loud_fail "Task context lost global New chat"
grep -Eq '(!)?(●|○)[0-9]+/[0-9?]+.*⊳[0-9]+' <<<"$task_full" \
  || loud_fail "Task context lost packed cached agent/ready pulse: $task_full"
! grep -Eq 'disk ok|D[0-9]+G' <<<"$task_full" \
  || loud_fail "packed lifecycle pulse leaked an unapproved disk segment: $task_full"
capture >"$scratch/wide-task-full.txt"

# Chat exposes only contextual chat controls and a fully-labelled fixed action.
tmux send-keys -t "$session" 0
wait_screen "[ New chat ]"
assert_one_context_row "$chat_context"
chat_screen=$(capture)
grep -qF "[ New chat ]" <<<"$chat_screen" || loud_fail "fully-labelled New chat missing"
! grep -qF "$task_context" <<<"$chat_screen" || loud_fail "task identity leaked into Chat context"
capture >"$scratch/wide-chat-full.txt"

# Medium keeps the labelled action; Full on a Termux-width phone uses its
# existing compact primary-action glyph. Optional controls collapse rather
# than creating a second row.
for spec in "76 30 medium" "40 22 termux"; do
  set -- $spec
  # Exercise a resize burst before settling at each measured viewport.
  tmux resize-window -t "$session" -x 58 -y 24
  tmux resize-window -t "$session" -x 104 -y 32
  tmux resize-window -t "$session" -x "$1" -y "$2"
  sleep 0.25
  if (( $1 < 60 )); then
    wait_screen "⊞"
  else
    wait_screen "[ New chat ]"
  fi
  assert_one_context_row "$chat_context"
  capture >"$scratch/$3-chat.txt"
done

# Keyboard-authoritative live preview and rollback remain available with mouse
# disabled. Layout mode replaces the same context row.
open_layout
layout_screen=$(capture)
[[ $(grep -Ec 'Dock:.*Size:.*Pending:|[[:space:]]P:(Split|Full|Hidden)' <<<"$layout_screen") == 1 ]] \
  || loud_fail "layout editor added rows or lost pending state: $layout_screen"
grep -qF '67%' <<<"$layout_screen" || loud_fail "phone layout lost current percentage: $layout_screen"
grep -qF 'Esc' <<<"$layout_screen" || loud_fail "phone layout lost Cancel: $layout_screen"
grep -qF 'Tab' <<<"$layout_screen" || loud_fail "phone layout lost keyboard-reachable More paging: $layout_screen"
tmux send-keys -t "$session" h '+' Escape
sleep 0.15
# Re-open and commit; narrow→wide restoration must retain desired state.
open_layout; tmux send-keys -t "$session" h '=' Enter
wait_layout dock left
wait_layout mode split
tmux resize-window -t "$session" -x 160 -y 44
sleep 0.25
wait_screen "$chat_context"
[[ $(layout_field dock) == left ]] || loud_fail "wide restoration lost desired dock"

[[ $(sha256sum "$G/graph.jsonl" | cut -d' ' -f1) == "$graph_before" ]] \
  || loud_fail "TUI layout mutated graph"
[[ $(sha256sum "$G/config.toml" | cut -d' ' -f1) == "$config_before" ]] \
  || loud_fail "TUI layout mutated config"

echo "PASS: first-use row distinguishes exact dock keys from 5% sizing/presets; phone paging, rollback/apply, one seam, responsive restoration, and non-mutating startup hold"
