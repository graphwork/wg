#!/usr/bin/env bash
# Stateful live-chat contract: a route-less chat under a Pi profile persists
# the exact Pi route, TUI restart reattaches the saved tmux/session without
# creating another graph row, CLI liveness agrees, and a dead pane can be
# resurrected through the real TUI recovery key.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
unset WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER
# Smoke gates often run from inside a worker tmux pane. Do not reuse that
# server's stale global PATH; the fixture owns an isolated TMUX_TMPDIR.
unset TMUX
command -v tmux >/dev/null 2>&1 \
    || loud_skip "MISSING TMUX" "tmux is required for the real TUI restart flow"
command -v python3 >/dev/null 2>&1 \
    || loud_skip "MISSING PYTHON3" "python3 is required to inspect graph/status JSON"

scratch=$(make_scratch)
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
export TMUX_TMPDIR="$scratch/tmux"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$TMUX_TMPDIR"
cd "$scratch"
G="$scratch/.wg"
WG_BIN=$(command -v wg)

fakebin="$scratch/fakebin"
mkdir -p "$fakebin"
pi_log="$scratch/pi-invocations.log"
cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -u
printf 'ARGV:' >>"$PI_STATEFUL_LOG"
printf ' %q' "$@" >>"$PI_STATEFUL_LOG"
printf '\n' >>"$PI_STATEFUL_LOG"
session_id=""
session_dir=""
args=("$@")
for ((i=0; i<${#args[@]}; i++)); do
    case "${args[$i]}" in
        --session-id) session_id="${args[$((i+1))]:-}" ;;
        --session-dir) session_dir="${args[$((i+1))]:-}" ;;
    esac
done
if [[ "$session_id" != "chat-0" || -z "$session_dir" ]]; then
    echo "PI_BAD_SESSION id=$session_id dir=$session_dir"
    exit 2
fi
mkdir -p "$session_dir"
continuity="$session_dir/stateful-continuity"
if [[ -f "$continuity" ]]; then
    echo "PI_HISTORY_CONTINUED:$(cat "$continuity")"
else
    printf 'saved-turn-chat-0\n' >"$continuity"
    echo "PI_HISTORY_STARTED"
fi
echo "PI_STATEFUL_READY"
while IFS= read -r line; do
    echo "PI_STATEFUL_ECHO:$line"
done
SH
chmod +x "$fakebin/pi"
export PATH="$fakebin:$PATH"
export PI_STATEFUL_LOG="$pi_log"

"$WG_BIN" --dir "$G" init --no-agency >init.log 2>&1 \
    || loud_fail "wg init failed: $(tail -20 init.log)"
cat >"$G/config.toml" <<'TOML'
[dispatcher]
model = "pi:openai-codex:gpt-5.6-sol"
TOML

# No --model/--executor: this must resolve the effective Pi profile route,
# never the serde/legacy Claude default.
"$WG_BIN" --dir "$G" chat create --name stateful --json >create.json 2>&1 \
    || loud_fail "route-less Pi chat create failed: $(cat create.json)"
python3 - "$G/graph.jsonl" <<'PY'
import json, sys
chats=[]
for line in open(sys.argv[1], encoding="utf-8"):
    node=json.loads(line)
    if node.get("id", "").startswith(".chat-"):
        chats.append(node)
assert len(chats) == 1, chats
chat=chats[0]
assert chat.get("id") == ".chat-0", chat
assert chat.get("executor_preset_name") == "pi", chat
assert chat.get("model") == "pi:openai-codex:gpt-5.6-sol", chat
argv=chat.get("command_argv", [])
assert argv and argv[0] == "pi" and "claude" not in argv, chat
PY
printf '%s\n' '{"active_coordinator_id":0,"right_panel_tab":"Chat","open_tabs":[".chat-0"],"active":".chat-0"}' >"$G/tui-state.json"

project_tag=$(basename "$scratch" | tr ':.' '--')
path_hash=$(printf '%s' "$(realpath "$G")" | sha256sum | cut -c1-16)
inner="wg-chat-${project_tag}-${path_hash}-chat-0"
outer="wgsmoke-stateful-$$"
outer2="${outer}-restart"
outer3="${outer}-empty-first-use"
empty_inner=""
cleanup_sessions() {
    tmux kill-session -t "$outer" 2>/dev/null || true
    tmux kill-session -t "$outer2" 2>/dev/null || true
    tmux kill-session -t "$outer3" 2>/dev/null || true
    tmux kill-session -t "$inner" 2>/dev/null || true
    tmux kill-session -t "$empty_inner" 2>/dev/null || true
}
add_cleanup_hook cleanup_sessions

launch_tui() {
    local session="$1"
    tmux new-session -d -s "$session" -x 170 -y 48 \
        "env PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TMUX_TMPDIR='$TMUX_TMPDIR' PI_STATEFUL_LOG='$PI_STATEFUL_LOG' '$WG_BIN' --dir '$G' tui --no-mouse"
}
capture() { tmux capture-pane -p -t "$1" 2>/dev/null || true; }
wait_screen() {
    local session="$1" pattern="$2"
    for _ in $(seq 1 120); do
        capture "$session" | grep -q "$pattern" && return 0
        sleep 0.1
    done
    return 1
}
chat_count() {
    python3 - "$G/graph.jsonl" <<'PY'
import json, sys
print(sum(1 for line in open(sys.argv[1], encoding="utf-8") if json.loads(line).get("id", "").startswith(".chat-")))
PY
}
assert_cli_live() {
    "$WG_BIN" --dir "$G" chat show 0 --json >"$scratch/show.json" \
        || loud_fail "chat show failed: $(cat "$scratch/show.json" 2>/dev/null)"
    python3 - "$scratch/show.json" <<'PY'
import json, sys
s=json.load(open(sys.argv[1]))
assert s["runtime_status"] == "supervised", s
assert s["tmux"]["live"] is True, s
PY
}

launch_tui "$outer"
wait_screen "$outer" 'PI_STATEFUL_READY' \
    || loud_fail "initial saved Pi pane did not start: $(capture "$outer" | tail -20) log=$(cat "$pi_log" 2>/dev/null)"
[[ "$(chat_count)" == 1 ]] || loud_fail "first TUI startup created a duplicate chat row"
inner_pid_before=$(tmux display-message -p -t "$inner" '#{pane_pid}')
assert_cli_live
payload1="before-restart-$$"
tmux send-keys -t "$outer" "$payload1" Enter
wait_screen "$outer" "PI_STATEFUL_ECHO:$payload1" \
    || loud_fail "initial Pi pane did not accept input"

# Kill only the human-facing outer TUI. The exact inner Pi session must remain,
# and the next TUI must use tui-state.json to reattach it rather than create.
tmux kill-session -t "$outer"
sleep 0.4
tmux has-session -t "$inner" 2>/dev/null \
    || loud_fail "persistent Pi session died with outer TUI"
launch_tui "$outer2"
wait_screen "$outer2" 'PI_STATEFUL_READY' \
    || loud_fail "restarted TUI did not reattach saved Pi pane: $(capture "$outer2" | tail -20)"
inner_pid_after=$(tmux display-message -p -t "$inner" '#{pane_pid}')
[[ "$inner_pid_after" == "$inner_pid_before" ]] \
    || loud_fail "restart replaced Pi session instead of reattaching: before=$inner_pid_before after=$inner_pid_after"
[[ "$(chat_count)" == 1 ]] \
    || loud_fail "TUI restart auto-created a duplicate chat row: count=$(chat_count)"
[[ $(wc -l <"$pi_log") -eq 1 ]] \
    || loud_fail "restart launched Pi again instead of reattaching: $(cat "$pi_log")"
payload2="after-restart-$$"
tmux send-keys -t "$outer2" "$payload2" Enter
wait_screen "$outer2" "PI_STATEFUL_ECHO:$payload2" \
    || loud_fail "reattached Pi pane did not accept input"
assert_cli_live

# Real resurrection flow: kill the persistent pane, wait for the visible death
# panel, then press its advertised R action. The saved route/session directory
# must spawn again and continue the same history marker.
tmux kill-session -t "$inner"
wait_screen "$outer2" 'Press R' \
    || loud_fail "dead Pi pane did not surface an actionable recovery panel: $(capture "$outer2" | tail -20)"
tmux send-keys -t "$outer2" R
wait_screen "$outer2" 'PI_HISTORY_CONTINUED:saved-turn-chat-0' \
    || loud_fail "TUI recovery did not resurrect the saved Pi session: $(capture "$outer2" | tail -20) log=$(cat "$pi_log")"
[[ "$(chat_count)" == 1 ]] || loud_fail "resurrection created a new graph chat"
payload3="after-resurrection-$$"
tmux send-keys -t "$outer2" "$payload3" Enter
wait_screen "$outer2" "PI_STATEFUL_ECHO:$payload3" \
    || loud_fail "resurrected Pi pane stayed dead/silent"
assert_cli_live

# The persisted selector remains exact throughout all phases.
python3 - "$G/tui-state.json" <<'PY'
import json, sys
s=json.load(open(sys.argv[1]))
assert s["active_coordinator_id"] == 0, s
assert s.get("active") == ".chat-0", s
PY

# Truly empty first-use project: the authoritative Empty publication may create
# exactly one chat, and route resolution must use the effective Pi profile.
# End the prior TUI/pane first so this phase also proves there is no accidental
# cross-project tmux/session reuse.
tmux kill-session -t "$outer2" 2>/dev/null || true
tmux kill-session -t "$inner" 2>/dev/null || true
sleep 0.3
empty_root="$scratch/empty"
empty_g="$empty_root/.wg"
mkdir -p "$empty_root"
"$WG_BIN" --dir "$empty_g" init --no-agency >"$scratch/empty-init.log" 2>&1 \
    || loud_fail "empty-project init failed: $(cat "$scratch/empty-init.log")"
cat >"$empty_g/config.toml" <<'TOML'
[dispatcher]
model = "pi:openai-codex:gpt-5.6-sol"
TOML
empty_trace="$scratch/empty-startup.jsonl"
empty_hash=$(printf '%s' "$(realpath "$empty_g")" | sha256sum | cut -c1-16)
empty_inner="wg-chat-empty-${empty_hash}-chat-0"
tmux new-session -d -s "$outer3" -x 170 -y 48 \
    "env PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TMUX_TMPDIR='$TMUX_TMPDIR' PI_STATEFUL_LOG='$PI_STATEFUL_LOG' WG_TUI_STARTUP_TRACE='$empty_trace' '$WG_BIN' --dir '$empty_g' tui --no-mouse"
wait_screen "$outer3" 'PI_STATEFUL_READY' \
    || loud_fail "empty project did not auto-create/start its profile-aware Pi chat: $(capture "$outer3" | tail -20) pi=$(cat "$pi_log" 2>/dev/null) graph=$(cat "$empty_g/graph.jsonl" 2>/dev/null) state=$(cat "$empty_g/tui-state.json" 2>/dev/null) trace=$(cat "$empty_trace" 2>/dev/null)"
python3 - "$empty_g/graph.jsonl" <<'PY'
import json, sys
chats=[json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if json.loads(line).get("id", "").startswith(".chat-")]
assert len(chats) == 1, chats
assert chats[0].get("executor_preset_name") == "pi", chats[0]
assert chats[0].get("model") == "pi:openai-codex:gpt-5.6-sol", chats[0]
assert "claude" not in chats[0].get("command_argv", []), chats[0]
PY

echo "PASS: route-less Pi chat persisted exact route; TUI restart reattached chat-0 with zero new rows; CLI/tmux liveness agreed; dead pane resurrected with continuous history; authoritative empty first-use auto-created one Pi chat"
