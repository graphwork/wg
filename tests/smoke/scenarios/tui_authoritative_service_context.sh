#!/usr/bin/env bash
# Candidate-binary real tmux/SGR flow for authenticated service/project context.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null 2>&1 || loud_skip "MISSING CARGO" "cargo is required"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"
command -v getent >/dev/null 2>&1 || loud_skip "MISSING GETENT" "OS account-home lookup is required"

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

# Put both canonical projects directly under the account database's home. The
# daemon/TUI inherit forged shell identity below; only getpwuid/gethostname from
# the service handshake may produce the visible destination and `~` contraction.
os_home=$(getent passwd "$(id -u)" | awk -F: '{print $6}')
[[ -n "$os_home" && -d "$os_home" && -w "$os_home" ]] \
  || loud_skip "ACCOUNT HOME UNWRITABLE" "cannot create canonical service fixtures under $os_home"
name_a="wgsa$$"
name_b="wgsb$$"
root_a="$os_home/$name_a"
root_b="$os_home/$name_b"
G1="$root_a/.wg"
G2="$root_b/.wg"
export TMUX_TMPDIR="$scratch/tmux"
mkdir -p "$root_a" "$root_b" "$scratch/fake-home" "$scratch/foreign/nested/cwd" "$TMUX_TMPDIR"

sessions=("wg-svc-a1-$$" "wg-svc-a2-$$" "wg-svc-b-$$")
clean_env=(env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_SPAWN_EPOCH -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER)
cleanup_all() {
  for session in "${sessions[@]}"; do tmux kill-session -t "$session" 2>/dev/null || true; done
  "${clean_env[@]}" "$WG_BIN" --dir "$G1" service stop >/dev/null 2>&1 || true
  "${clean_env[@]}" "$WG_BIN" --dir "$G2" service stop >/dev/null 2>&1 || true
  rm -rf "$root_a" "$root_b"
}
add_cleanup_hook cleanup_all

for graph in "$G1" "$G2"; do
  "${clean_env[@]}" "$WG_BIN" --dir "$graph" init --no-agency >/dev/null
  cat >"$graph/config.toml" <<'TOML'
[dispatcher]
model = "pi:openrouter:example/model"
TOML
  "${clean_env[@]}" "$WG_BIN" --dir "$graph" chat create --name destination --command cat >/dev/null
  cat >"$graph/tui-state.json" <<'JSON'
{"layout":{"dock":"right","size_percent":67,"mode":"full"},"active_coordinator_id":0,"right_panel_tab":"Dashboard","open_tabs":[".chat-0"],"active":".chat-0"}
JSON
done
# Deliberately forged shell identity must not enter the authenticated handshake.
for graph in "$G1" "$G2"; do
  USER=forged-user HOSTNAME=forged-host HOME="$scratch/fake-home" \
    "${clean_env[@]}" "$WG_BIN" --dir "$graph" service start >/dev/null
 done

read -r service_user service_host service_home protocol <<<"$(python3 - "$G1/service/state.json" <<'PY'
import json,sys
s=json.load(open(sys.argv[1])); i=s["identity"]
print(i["service_user"], i["service_host"], i.get("service_home",""), i["protocol"])
PY
)"
[[ "$service_user" != forged-user && "$service_host" != forged-host ]] \
  || loud_fail "inherited USER/HOSTNAME leaked into authenticated identity"
[[ "$service_home" == "$os_home" ]] || loud_fail "service home is not OS-account authoritative: $service_home"
[[ "$protocol" == worksgood-service-identity-v1 ]] || loud_fail "unexpected protocol: $protocol"
expected_a="$service_user@$service_host:~/$name_a"
expected_b="$service_user@$service_host:~/$name_b"

launch_tui() {
  local session=$1 graph=$2 cwd=$3
  tmux new-session -d -s "$session" -x 300 -y 32 \
    "cd '$cwd' && env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_SPAWN_EPOCH -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER HOME='$scratch/fake-home' USER=other-forged-user HOSTNAME=other-forged-host XDG_CONFIG_HOME='$scratch/fake-home/.config' WG_GLOBAL_DIR='$scratch/fake-home/.wg' TERM=xterm-256color MOSH_IP=192.0.2.1 '$WG_BIN' --dir '$graph' tui"
  tmux set-option -t "$session" mouse on
  tmux resize-window -t "$session" -x 300 -y 32
}
# Two clients on alpha plus a beta client, each launched from an unrelated or
# other-project cwd, pin same-host/multi-project/multi-client/nested-cwd cases.
launch_tui "${sessions[0]}" "$G1" "$scratch/foreign/nested/cwd"
launch_tui "${sessions[1]}" "$G1" "$root_b"
launch_tui "${sessions[2]}" "$G2" "$root_a"

capture() { tmux capture-pane -p -t "$1" 2>/dev/null || true; }
wait_screen() {
  local session=$1 needle=$2 label=${3:-"screen missing $needle"}
  for _ in $(seq 1 300); do capture "$session" | grep -Fq "$needle" && return 0; sleep 0.03; done
  loud_fail "$label: $(capture "$session" | tr '\n' '|')"
}
coord() {
  local session=$1 needle=$2
  capture "$session" | python3 -c 'import sys
needle=sys.argv[1]
for y,row in enumerate(sys.stdin.read().splitlines(),1):
    x=row.find(needle)
    if x >= 0:
        print(x+1,y); raise SystemExit(0)
raise SystemExit(1)' "$needle"
}
click_xy() {
  local session=$1 x=$2 y=$3
  tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM\033[<0;%s;%sm' "$x" "$y" "$x" "$y")"
}

wait_screen "${sessions[0]}" "$expected_a" "alpha client 1 did not show authoritative destination"
wait_screen "${sessions[1]}" "$expected_a" "alpha client 2 did not agree on destination"
wait_screen "${sessions[2]}" "$expected_b" "beta client confused same-host project services"
# Command-chat startup may atomically normalize its pre-existing route once;
# pin after that unrelated baseline. Every identity poll, click, detail open,
# resize, and daemon restart below must leave graph bytes unchanged.
sleep 2
graph_hash_a=$(sha256sum "$G1/graph.jsonl")
graph_hash_b=$(sha256sum "$G2/graph.jsonl")
for session in "${sessions[@]}"; do
  screen=$(capture "$session")
  [[ "$screen" != *forged-user* && "$screen" != *forged-host* && "$screen" != *foreign/nested/cwd* ]] \
    || loud_fail "$session rendered inherited client shell identity: $screen"
done

# Click the real, current destination cells and inspect the complete copyable
# handshake identity. The graph and socket are absolute, while the bar is clean.
tmux resize-window -t "${sessions[0]}" -x 300 -y 32
wait_screen "${sessions[0]}" "$expected_a" "settled wide frame omitted alpha destination"
xy=$(coord "${sessions[0]}" "$expected_a") || loud_fail "visible alpha identity had no coordinate"
read -r identity_x identity_y <<<"$xy"
click_xy "${sessions[0]}" "$identity_x" "$identity_y"
wait_screen "${sessions[0]}" "Workspace / Service Details" "identity click did not open details"
for expected in "$G1" "$G1/service/daemon.sock" "worksgood-service-identity-v1" "pi:openrouter:example/model" "PID birth:" "Compatible build:"; do
  wait_screen "${sessions[0]}" "$expected" "details missing $expected"
done

# Direct keyboard parity reaches the identical read-only surface.
tmux send-keys -t "${sessions[0]}" Escape I
wait_screen "${sessions[0]}" "Workspace / Service Details" "command-mode I did not open service details"
tmux send-keys -t "${sessions[0]}" Escape

# Record a wide-frame coordinate, rotate through medium/phone/Termux widths,
# and prove optional context degrades away before exact chat identity/primary
# lanes/New control. A tap at the clipped old coordinate must not reopen detail.
old_x=$identity_x; old_y=$identity_y
for width in 140 100 80 60 40 32; do
  tmux resize-window -t "${sessions[0]}" -x "$width" -y 24
  row=""
  for _ in $(seq 1 100); do
    row=$(capture "${sessions[0]}" | grep -m1 -F '↯' || true)
    [[ -n "$row" ]] && break
    sleep 0.03
  done
  if [[ "$row" != *'.chat-0'* ]]; then
    lane_xy=$(coord "${sessions[0]}" '↯' || true)
    if [[ -n "$lane_xy" ]]; then
      read -r lane_x lane_y <<<"$lane_xy"; click_xy "${sessions[0]}" "$lane_x" "$lane_y"; sleep 0.1
      row=$(capture "${sessions[0]}" | grep -m1 -F '↯' || true)
    fi
  fi
  [[ "$row" == *'.chat-0'* && "$row" == *'↯'* && "$row" == *'⌁'* && "$row" == *'⌂'* ]] \
    || loud_fail "width $width crowded exact identity/context lanes: $row"
  [[ "$row" == *'⊞'* || "$row" == *'⌕'* || "$row" == *'Panel'* || "$row" == *'p Pane'* || "$row" == *'Split'* || "$row" == *"$service_user@$service_host:"* ]] \
    || loud_fail "width $width lost every primary row action: $row"
done
if (( old_x <= 20 )); then
  click_xy "${sessions[0]}" "$old_x" "$old_y"
  sleep 0.1
  capture "${sessions[0]}" | grep -Fq "Workspace / Service Details" \
    && loud_fail "clipped old identity coordinate remained active"
fi

# Restart the daemon underneath both alpha clients. The next bounded handshake
# must install the new PID birth; details may never remain pinned to the old one.
old_birth=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid_start_identity"])' "$G1/service/state.json")
"${clean_env[@]}" "$WG_BIN" --dir "$G1" service stop >/dev/null
"${clean_env[@]}" "$WG_BIN" --dir "$G1" service start >/dev/null
new_birth=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid_start_identity"])' "$G1/service/state.json")
[[ "$new_birth" != "$old_birth" ]] || loud_fail "service restart did not change PID birth identity"
tmux resize-window -t "${sessions[0]}" -x 220 -y 32
wait_screen "${sessions[0]}" "$expected_a" "destination did not recover after restart"
for _ in $(seq 1 120); do
  xy=$(coord "${sessions[0]}" "$expected_a" 2>/dev/null || true)
  if [[ -n "$xy" ]]; then
    read -r x y <<<"$xy"; click_xy "${sessions[0]}" "$x" "$y"; sleep 0.05
    if capture "${sessions[0]}" | grep -Fq "$new_birth"; then break; fi
    tmux send-keys -t "${sessions[0]}" Escape
  fi
  sleep 0.05
done
capture "${sessions[0]}" | grep -Fq "$new_birth" \
  || loud_fail "fresh service identity/PID birth never replaced restarted session"

[[ "$(sha256sum "$G1/graph.jsonl")" == "$graph_hash_a" ]] || loud_fail "identity retrieval/TUI mutated alpha graph"
[[ "$(sha256sum "$G2/graph.jsonl")" == "$graph_hash_b" ]] || loud_fail "identity retrieval/TUI mutated beta graph"

echo "PASS: authenticated multi-project/multi-client destination, exact width degradation, click/keyboard parity, full details, and restart fencing"
