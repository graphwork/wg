#!/usr/bin/env bash
# Real tmux/PTY screenshots for typed autonomous/plumbing presentation.
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

export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
unset TMUX TMUX_TMPDIR WG_DIR WG_TASK_ID WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$scratch/project"
G="$scratch/project/.wg"
"$WG_BIN" --dir "$G" init --no-agency >/dev/null
cat >"$G/config.toml" <<'TOML'
[agency]
auto_place = false
auto_assign = false
auto_evaluate = false
flip_enabled = false
[tui]
show_system_tasks = false
show_running_system_tasks = false
TOML

# Prefixes intentionally disagree with policy. If any view infers visibility
# from the ID, these assertions fail: the .assign-* row is autonomous/visible,
# while the ordinary-looking helper is typed plumbing/hidden.
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
p=sys.argv[1]
rows=[
 {"kind":"task","id":"parent","title":"Primary parent","status":"open","presentation":"primary","origin":{"kind":"user"}},
 {"kind":"task","id":".quality-pass-smoke","title":"Autonomous quality pass","status":"open","after":["parent"],"presentation":"autonomous","origin":{"kind":"autonomous-actor","parent_task":"parent","goal":"raise release quality"}},
 {"kind":"task","id":".assign-visible-by-policy","title":"Autonomous synthesis work","status":"open","after":["parent"],"presentation":"autonomous","origin":{"kind":"autonomous-actor","parent_task":"parent","goal":"synthesize new source work"}},
 {"kind":"task","id":"ordinary-plumbing-helper","title":"Hidden active verifier","status":"in-progress","after":["parent"],"presentation":"plumbing","origin":{"kind":"agency-plumbing","parent_task":"parent","goal":"verification satellite"}},
 {"kind":"task","id":".evaluate-parent","title":"Queued evaluator","status":"open","after":["parent"],"presentation":"plumbing","origin":{"kind":"agency-plumbing","parent_task":"parent","goal":"evaluation satellite"}},
]
with open(p,'w') as f:
 for row in rows: f.write(json.dumps(row,separators=(',',':'))+'\n')
PY

assert_default_view() {
  local format=$1; shift
  local out
  out=$(NO_COLOR=1 "$WG_BIN" --dir "$G" viz --all --no-tui "$@")
  grep -Fq '.quality-pass-smoke' <<<"$out" || loud_fail "$format hid autonomous quality work: $out"
  grep -Fq '· .quality-pass-smoke' <<<"$out" || loud_fail "$format omitted centered-dot actor glyph: $out"
  grep -Fq '· .assign-visible-by-policy' <<<"$out" || loud_fail "$format inferred .assign visibility from its name: $out"
  ! grep -Fq 'ordinary-plumbing-helper' <<<"$out" || loud_fail "$format exposed typed plumbing: $out"
  ! grep -Fq '.evaluate-parent' <<<"$out" || loud_fail "$format exposed evaluator plumbing: $out"
}
assert_default_view ASCII
assert_default_view DOT --dot
assert_default_view Mermaid --mermaid
all=$(NO_COLOR=1 "$WG_BIN" --dir "$G" viz --all --show-internal --no-tui)
grep -Fq ordinary-plumbing-helper <<<"$all" || loud_fail "show-internal did not reveal ordinary-ID plumbing"
grep -Fq .evaluate-parent <<<"$all" || loud_fail "show-internal did not reveal evaluator plumbing"

session="wg-autonomous-visibility-$$"
cleanup_session() { tmux kill-session -t "$session" 2>/dev/null || true; }
add_cleanup_hook cleanup_session
capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
wait_screen() {
  local needle=$1 label=$2
  for _ in $(seq 1 320); do
    capture | grep -Fq "$needle" && return 0
    sleep 0.025
  done
  loud_fail "$label: $(capture | tr '\n' '|')"
}
wait_absent() {
  local needle=$1 label=$2
  for _ in $(seq 1 320); do
    ! capture | grep -Fq "$needle" && return 0
    sleep 0.025
  done
  loud_fail "$label: $(capture | tr '\n' '|')"
}
coord() {
  local needle=$1
  capture | python3 -c 'import sys
needle=sys.argv[1]
for y,row in enumerate(sys.stdin.read().splitlines(),1):
 x=row.find(needle)
 if x>=0:
  print(x+1,y); raise SystemExit(0)
raise SystemExit(1)' "$needle"
}
mouse_click() {
  local x=$1 y=$2
  tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM\033[<0;%s;%sm' "$x" "$y" "$x" "$y")"
}

tmux new-session -d -s "$session" -x 200 -y 36 \
  "cd '$scratch/project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$G' tui"
tmux set-option -t "$session" mouse on
wait_screen '· plumbing: hidden · 2 hidden' 'default centered-dot plumbing control missing'
wait_screen '· .quality-pass-smoke' 'autonomous quality actor missing from default TUI'
wait_screen '· .assign-visible-by-policy' 'typed autonomous .assign name missing from default TUI'
! capture | grep -Fq ordinary-plumbing-helper || loud_fail "default TUI exposed typed active plumbing"
! capture | grep -Fq .evaluate-parent || loud_fail "default TUI exposed queued evaluator"
capture >"$scratch/default-screen.txt"

# Keyboard cycle: hidden -> running only -> all.
tmux send-keys -t "$session" .
wait_screen '· plumbing: running only · 1 hidden' 'keyboard did not enter running-only mode'
wait_screen ordinary-plumbing-helper 'running-only mode did not reveal active plumbing'
! capture | grep -Fq .evaluate-parent || loud_fail "running-only mode revealed queued plumbing"
wait_screen '[∴ evaluating]' 'parent annotation did not summarize hidden active plumbing'
capture >"$scratch/running-screen.txt"

tmux send-keys -t "$session" .
wait_screen '· plumbing: all · 0 hidden' 'keyboard did not enter all mode'
wait_screen .evaluate-parent 'all mode did not reveal queued plumbing'
capture >"$scratch/all-screen.txt"

# Mouse cycle uses the same labeled control: all -> hidden.
read -r mx my < <(coord '· plumbing: all · 0 hidden')
mouse_click "$((mx+2))" "$my"
wait_screen '· plumbing: hidden · 2 hidden' 'mouse click did not cycle back to hidden'
for hidden in ordinary-plumbing-helper .evaluate-parent; do
  wait_absent "$hidden" "mouse-hidden mode still shows $hidden"
done
capture >"$scratch/mouse-hidden-screen.txt"

# Even while collapsed, the active helper remains navigable through its typed
# parent annotation into the real inspector.
wait_screen '[∴ evaluating]' 'hidden active plumbing lost its parent annotation'
read -r ax ay < <(coord '[∴ evaluating]')
mouse_click "$((ax+3))" "$ay"
wait_screen 'ordinary-plumbing-helper' 'annotation click did not navigate to hidden helper detail'
capture >"$scratch/hidden-helper-inspector-screen.txt"

echo "PASS: typed origin/presentation drives ASCII/DOT/Mermaid/TUI; centered-dot control keyboard+mouse screenshots cover hidden/running/all and hidden-helper inspector navigation"
