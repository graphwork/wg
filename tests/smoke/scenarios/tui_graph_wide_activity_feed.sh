#!/usr/bin/env bash
# Candidate-binary tmux proof for the primary graph-wide Workspace Activity feed.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
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

export HOME="$scratch/home" XDG_CONFIG_HOME="$HOME/.config" WG_GLOBAL_DIR="$HOME/.wg"
unset TMUX TMUX_TMPDIR WG_DIR WG_TASK_ID WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$scratch/project"
G="$scratch/project/.wg"
"$WG_BIN" --dir "$G" init --no-agency >/dev/null
"$WG_BIN" --dir "$G" add seeded-history -d "safe activity seed" >/dev/null
cat >"$G/tui-state.json" <<'JSON'
{"layout":{"dock":"right","size_percent":70,"mode":"full"},"active_coordinator_id":0,"right_panel_tab":"Chat","open_tabs":[],"active":""}
JSON

session="wg-activity-feed-$$"
cleanup_session() { tmux kill-session -t "$session" 2>/dev/null || true; }
add_cleanup_hook cleanup_session
capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
wait_screen() {
  local needle=$1 label=${2:-"missing $1"}
  for _ in $(seq 1 240); do capture | grep -Fq "$needle" && return 0; sleep 0.04; done
  loud_fail "$label: $(capture | tr '\n' '|')"
}
coord() {
  capture | python3 -c 'import sys
needle=sys.argv[1]
for y,row in enumerate(sys.stdin.read().splitlines(),1):
 x=row.find(needle)
 if x>=0: print(x+1,y); raise SystemExit
raise SystemExit(1)' "$1"
}
click_text() {
  local xy x y
  xy=$(coord "$1") || loud_fail "click target $1 missing"
  read -r x y <<<"$xy"
  tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM\033[<0;%s;%sm' "$x" "$y" "$x" "$y")"
}

tmux new-session -d -s "$session" -x 110 -y 28 \
  "cd '$scratch/project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$G' tui"
tmux set-option -t "$session" mouse on
wait_screen "⌂" "Workspace glyph did not render"
click_text "⌂"
wait_screen "⌂ Activity" "one Workspace activation did not land on Activity"
wait_screen "seeded-history" "bounded recent graph history was not immediate"
capture | grep -Eq '[0-9]+[smhd] · [0-9]{2}:[0-9]{2}:[0-9]{2}' \
  || loud_fail "relative age and local/system clock are not both visible: $(capture | tr '\n' '|')"

# Idle repaint advances relative age while the clock remains present.
before_age=$(capture | grep 'seeded-history' | grep -oE '[0-9]+[smhd]' | head -1 || true)
sleep 2
after_age=$(capture | grep 'seeded-history' | grep -oE '[0-9]+[smhd]' | head -1 || true)
[[ -n "$before_age" && -n "$after_age" && "$before_age" != "$after_age" ]] \
  || loud_fail "relative age did not advance on an idle TUI ($before_age -> $after_age)"

"$WG_BIN" --dir "$G" edit seeded-history --title "live-tail-one" >/dev/null
wait_screen "task edited" "new activity did not live-tail"

# Home freezes exact viewport; a later event is counted but does not steal it.
tmux send-keys -t "$session" Home
wait_screen "paused" "scroll-up did not freeze activity viewport"
"$WG_BIN" --dir "$G" edit seeded-history --title "live-tail-two" >/dev/null
wait_screen "unseen" "frozen viewport did not show unseen-event count"
tmux send-keys -t "$session" End
wait_screen "· live" "End did not resume live tail"

# Compact Workspace activation has the same primary destination.
tmux resize-window -t "$session" -x 40 -y 20
sleep 0.2
tmux send-keys -t "$session" Left # leave Activity via ordinary tab navigation
click_text "⌂"
wait_screen "⌂ Activity" "compact Workspace activation did not land on Activity"

# Repeated activation exposes all secondary Workspace owners.
click_text "⌂"
wait_screen "Dashboard" "Dashboard missing from Workspace actions"
wait_screen "Config" "Config missing from Workspace actions"
wait_screen "Raw service log" "raw Service log missing from Workspace actions"

echo "PASS: graph-wide Activity shows history, live tail, advancing relative+clock time, frozen unseen count, End follow, compact reachability, and secondary Workspace actions"
