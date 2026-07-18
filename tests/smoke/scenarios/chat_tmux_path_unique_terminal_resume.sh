#!/usr/bin/env bash
# Path-unique tmux ownership + terminal-safe resume human-flow regression.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
unset WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER TMUX
command -v tmux >/dev/null 2>&1 \
    || loud_skip "MISSING TMUX" "tmux is required for the real TUI ownership flow"
command -v python3 >/dev/null 2>&1 \
    || loud_skip "MISSING PYTHON3" "python3 is required to inspect graph/status JSON"

scratch=$(make_scratch)
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
export TMUX_TMPDIR="$scratch/tmux"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$TMUX_TMPDIR"
WG_BIN=$(command -v wg)

fakebin="$scratch/fakebin"
mkdir -p "$fakebin"
owner_log="$scratch/pi-owners.log"
cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -u
case "${WG_DIR:-}" in
    */a/shared/.wg) owner=A ;;
    */b/shared/.wg) owner=B ;;
    *) owner=OTHER ;;
esac
printf '%s:%s:%s\n' "$owner" "$$" "${WG_DIR:-missing}" >>"$PI_OWNER_LOG"
echo "PI_OWNER_${owner}_READY"
while IFS= read -r line; do echo "PI_OWNER_${owner}_ECHO:$line"; done
SH
chmod +x "$fakebin/pi"
export PATH="$fakebin:$PATH"
export PI_OWNER_LOG="$owner_log"

root_a="$scratch/a/shared"
root_b="$scratch/b/shared"
graph_a="$root_a/.wg"
graph_b="$root_b/.wg"
for root in "$root_a" "$root_b"; do
    mkdir -p "$root"
    "$WG_BIN" --dir "$root/.wg" init --no-agency >"$root/init.log" 2>&1 \
        || loud_fail "wg init failed for $root: $(cat "$root/init.log")"
    cat >"$root/.wg/config.toml" <<'TOML'
[dispatcher]
model = "pi:openai-codex:test-path-owner"
TOML
    "$WG_BIN" --dir "$root/.wg" chat create --name owner --json >"$root/create.json" 2>&1 \
        || loud_fail "chat create failed for $root: $(cat "$root/create.json")"
    printf '%s\n' '{"active_coordinator_id":0,"right_panel_tab":"Chat","open_tabs":[".chat-0"],"active":".chat-0"}' >"$root/.wg/tui-state.json"
done

outer_a="wg-path-a-$$"
outer_a2="${outer_a}-restart"
outer_a3="${outer_a}-legacy"
outer_b="wg-path-b-$$"
terminal_session=""
cleanup_sessions() {
    tmux kill-session -t "$outer_a" 2>/dev/null || true
    tmux kill-session -t "$outer_a2" 2>/dev/null || true
    tmux kill-session -t "$outer_a3" 2>/dev/null || true
    tmux kill-session -t "$outer_b" 2>/dev/null || true
    [[ -z "$terminal_session" ]] || tmux kill-session -t "$terminal_session" 2>/dev/null || true
    tmux list-sessions -F '#{session_name}' 2>/dev/null | grep '^wg-chat-' | while IFS= read -r s; do
        env_line=$(tmux show-environment -t "$s" WG_DIR 2>/dev/null || true)
        case "$env_line" in
            "WG_DIR=$graph_a"|"WG_DIR=$graph_b") tmux kill-session -t "$s" 2>/dev/null || true ;;
        esac
    done
}
add_cleanup_hook cleanup_sessions

launch_tui() {
    local outer=$1 graph=$2
    tmux new-session -d -s "$outer" -x 150 -y 44 \
        "env PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TMUX_TMPDIR='$TMUX_TMPDIR' PI_OWNER_LOG='$PI_OWNER_LOG' '$WG_BIN' --dir '$graph' tui --no-mouse"
}
capture() { tmux capture-pane -p -t "$1" 2>/dev/null || true; }
wait_screen() {
    local outer=$1 marker=$2
    for _ in $(seq 1 150); do
        capture "$outer" | grep -q "$marker" && return 0
        sleep 0.1
    done
    return 1
}
session_from_show() {
    local graph=$1
    local out="$scratch/show-$(basename "$(dirname "$graph")")-$$.json"
    "$WG_BIN" --dir "$graph" chat show 0 --json >"$out" \
        || loud_fail "chat show failed for $graph: $(cat "$out" 2>/dev/null)"
    python3 - "$out" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["tmux"]["session"])
PY
}

# Graph A owns the first persistent pane.
launch_tui "$outer_a" "$graph_a"
wait_screen "$outer_a" 'PI_OWNER_A_READY' \
    || loud_fail "graph A TUI did not start its own Pi: $(capture "$outer_a" | tail -20)"
session_a=$(session_from_show "$graph_a")
tmux has-session -t "$session_a" 2>/dev/null \
    || loud_fail "graph A canonical session missing: $session_a"
pid_a=$(tmux display-message -p -t "$session_a" '#{pane_pid}')

# Same-basename graph B must not report or attach graph A's pane.
"$WG_BIN" --dir "$graph_b" chat show 0 --json >"$scratch/show-b-before.json"
python3 - "$scratch/show-b-before.json" <<'PY'
import json, sys
s=json.load(open(sys.argv[1]))
assert s["runtime_status"] == "dormant", s
assert s["tmux"]["live"] is False, s
PY
session_b=$(session_from_show "$graph_b")
[[ "$session_a" != "$session_b" ]] \
    || loud_fail "same-basename graphs collided on tmux session $session_a"
launch_tui "$outer_b" "$graph_b"
wait_screen "$outer_b" 'PI_OWNER_B_READY' \
    || loud_fail "graph B attached the wrong pane or failed to start: $(capture "$outer_b" | tail -20)"
tmux has-session -t "$session_b" 2>/dev/null \
    || loud_fail "graph B canonical session missing: $session_b"
[[ "$(tmux display-message -p -t "$session_a" '#{pane_pid}')" == "$pid_a" ]] \
    || loud_fail "graph B replaced graph A's tmux owner"

# Normal same-graph restart reattaches the exact path-unique owner.
tmux kill-session -t "$outer_a"
launch_tui "$outer_a2" "$graph_a"
wait_screen "$outer_a2" 'PI_OWNER_A_READY' \
    || loud_fail "graph A restart failed to reattach"
[[ "$(tmux display-message -p -t "$session_a" '#{pane_pid}')" == "$pid_a" ]] \
    || loud_fail "same-graph restart replaced Pi: before=$pid_a after=$(tmux display-message -p -t "$session_a" '#{pane_pid}')"
[[ "$(grep -c '^A:' "$owner_log")" == 1 ]] \
    || loud_fail "same-graph restart launched duplicate A Pi: $(cat "$owner_log")"

# Compatibility: a prior basename-only session with matching WG_DIR is renamed
# in place and reattached. The owner marker, not the basename, authorizes it.
tmux kill-session -t "$outer_a2"
tmux kill-session -t "$outer_b"
tmux kill-session -t "$session_b"
legacy="wg-chat-shared-chat-0"
tmux rename-session -t "$session_a" "$legacy" \
    || loud_fail "could not stage legacy session migration"
"$WG_BIN" --dir "$graph_b" chat show 0 --json >"$scratch/show-b-legacy.json"
python3 - "$scratch/show-b-legacy.json" <<'PY'
import json, sys
s=json.load(open(sys.argv[1]))
assert s["runtime_status"] == "dormant", s
assert s["tmux"]["live"] is False, s
PY
tmux has-session -t "$legacy" 2>/dev/null \
    || loud_fail "wrong graph renamed or consumed graph A's legacy session"
launch_tui "$outer_a3" "$graph_a"
wait_screen "$outer_a3" 'PI_OWNER_A_READY' \
    || loud_fail "legacy same-graph pane was not reattached"
tmux has-session -t "$session_a" 2>/dev/null \
    || loud_fail "owned legacy session was not migrated to $session_a"
! tmux has-session -t "$legacy" 2>/dev/null \
    || loud_fail "legacy basename-only session remained after migration"
[[ "$(tmux display-message -p -t "$session_a" '#{pane_pid}')" == "$pid_a" ]] \
    || loud_fail "legacy migration replaced the pane instead of renaming it"

# Terminal-safe resume: an archived/done/abandoned task plus a stale canonical
# tmux session must fail before SetChatExecutor and preserve graph/route/session.
terminal_root="$scratch/terminal"
terminal_graph="$terminal_root/.wg"
mkdir -p "$terminal_root"
"$WG_BIN" --dir "$terminal_graph" init --no-agency >"$terminal_root/init.log" 2>&1 \
    || loud_fail "terminal fixture init failed: $(cat "$terminal_root/init.log")"
cat >"$terminal_graph/config.toml" <<'TOML'
[dispatcher]
model = "pi:openai-codex:test-terminal"
TOML
"$WG_BIN" --dir "$terminal_graph" chat create --name terminal --json >"$terminal_root/create.json" 2>&1 \
    || loud_fail "terminal fixture chat create failed: $(cat "$terminal_root/create.json")"
terminal_session=$(session_from_show "$terminal_graph")
tmux new-session -d -s "$terminal_session" -e "WG_DIR=$terminal_graph" -- sleep 300 \
    || loud_fail "could not stage stale terminal tmux proof"
start_wg_daemon "$terminal_root" --max-agents 0

set_terminal_shape() {
    local status=$1 archived=$2
    python3 - "$terminal_graph/graph.jsonl" "$status" "$archived" <<'PY'
import json, os, sys, tempfile
path, status, archived = sys.argv[1:]
rows=[]
for line in open(path, encoding="utf-8"):
    row=json.loads(line)
    if row.get("id") == ".chat-0":
        row["status"] = status
        tags=[t for t in row.get("tags", []) if t != "archived"]
        if archived == "yes": tags.append("archived")
        row["tags"] = tags
    rows.append(row)
fd,tmp=tempfile.mkstemp(dir=os.path.dirname(path)); os.close(fd)
with open(tmp,"w",encoding="utf-8") as f:
    for row in rows: f.write(json.dumps(row,separators=(",",":"))+"\n")
os.replace(tmp,path)
PY
}
task_fingerprint() {
    python3 - "$terminal_graph/graph.jsonl" <<'PY'
import hashlib, json, sys
row=next(json.loads(line) for line in open(sys.argv[1]) if json.loads(line).get("id")==".chat-0")
print(hashlib.sha256(json.dumps(row,sort_keys=True,separators=(",",":")).encode()).hexdigest())
PY
}
route_fingerprint() {
    find "$terminal_graph" -type f \( -name '*coordinator*state*' -o -name '*chat*state*' \) -print0 2>/dev/null \
        | sort -z | xargs -0r sha256sum | sha256sum | awk '{print $1}'
}

for shape in done archived abandoned; do
    case "$shape" in
        done) set_terminal_shape done no ;;
        archived) set_terminal_shape done yes ;;
        abandoned) set_terminal_shape abandoned no ;;
    esac
    before_task=$(task_fingerprint)
    before_route=$(route_fingerprint)
    before_pid=$(tmux display-message -p -t "$terminal_session" '#{pane_pid}')
    before_set_count=$(grep -c 'IPC SetChatExecutor' "$terminal_graph/service/daemon.log" 2>/dev/null || true)
    set +e
    resume_out=$("$WG_BIN" --dir "$terminal_graph" chat resume 0 --json 2>&1)
    resume_rc=$?
    set -e
    [[ "$resume_rc" -ne 0 ]] \
        || loud_fail "$shape chat plus stale tmux falsely resumed: $resume_out"
    grep -qiE 'terminal|archived|abandoned|done' <<<"$resume_out" \
        || loud_fail "$shape rejection was not authoritative/clear: $resume_out"
    [[ "$(task_fingerprint)" == "$before_task" ]] \
        || loud_fail "$shape resume rejection mutated graph task"
    [[ "$(route_fingerprint)" == "$before_route" ]] \
        || loud_fail "$shape resume rejection mutated route/session metadata"
    [[ "$(tmux display-message -p -t "$terminal_session" '#{pane_pid}')" == "$before_pid" ]] \
        || loud_fail "$shape resume rejection mutated stale tmux session"
    after_set_count=$(grep -c 'IPC SetChatExecutor' "$terminal_graph/service/daemon.log" 2>/dev/null || true)
    [[ "$after_set_count" == "$before_set_count" ]] \
        || loud_fail "$shape resume reached SetChatExecutor despite terminal graph state"
done

echo "PASS: equal-basename graphs own distinct tmux panes; same-graph and owned-legacy restarts reattach; terminal chats reject stale runtime proof without mutation"
