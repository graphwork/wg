#!/usr/bin/env bash
# Real tmux/PTY proof that effective worker authority is visible in the TUI.
set -euo pipefail
source "$(dirname "$0")/_helpers.sh"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
: "${WG_BIN:?smoke harness must provide candidate WG_BIN}"
[[ -x $WG_BIN ]] || loud_fail "candidate WG_BIN is not executable: $WG_BIN"

scratch=$(mktemp -d "${TMPDIR:-/tmp}/wg-worker-control-tui.XXXXXX")
session="wg-worker-control-tui-$$"
cleanup() {
  tmux kill-session -t "$session" >/dev/null 2>&1 || true
  [[ ${WG_SMOKE_KEEP_TMP:-0} == 1 ]] || rm -rf "$scratch"
}
trap cleanup EXIT
project="$scratch/project"
home="$scratch/home"
mkdir -p "$project" "$home/.config"
(
  cd "$project"
  env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_CONTROL_PROTOCOL \
    -u WG_WORKER_IPC -u WG_WORKER_CONTROL_MODE HOME="$home" XDG_CONFIG_HOME="$home/.config" \
    "$WG_BIN" init --no-agency >/dev/null
  env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_CONTROL_PROTOCOL \
    -u WG_WORKER_IPC -u WG_WORKER_CONTROL_MODE HOME="$home" XDG_CONFIG_HOME="$home/.config" \
    WG_DIR="$project/.wg" "$WG_BIN" add "Worker control inspector" --id control-inspector >/dev/null
)

tmux new-session -d -s "$session" -x 180 -y 34 \
  "cd '$project' && env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_CONTROL_PROTOCOL -u WG_WORKER_IPC -u WG_WORKER_CONTROL_MODE HOME='$home' XDG_CONFIG_HOME='$home/.config' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$project/.wg' tui"
capture() { tmux capture-pane -p -t "$session" 2>/dev/null || true; }
wait_for() {
  local needle=$1
  for _ in $(seq 1 400); do
    capture | grep -Fq "$needle" && return 0
    sleep 0.025
  done
  loud_fail "TUI omitted '$needle': $(capture | tr '\n' '|')"
}
wait_for 'control-inspector'
# Exercise the real key dispatcher rather than only reading a static snapshot.
tmux send-keys -t "$session" Home Enter
wait_for '── Worker control ──'
wait_for 'Effective mode: trusted'
wait_for 'Preflight: wg capabilities'
capture >"$scratch/worker-control-tui.txt"

echo "PASS: real tmux/TUI inspector rendered effective trusted worker-control mode, restrictions, and capability preflight after keyboard input"
