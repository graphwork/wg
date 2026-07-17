#!/usr/bin/env bash
# Real human-flow regression: opening an empty/terminal-only TUI is graph-only.
# Creation starts only after command-mode n or the labeled pointer control.

set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v tmux >/dev/null 2>&1 \
    || loud_skip "MISSING TMUX" "tmux is required for the real TUI flow"
command -v python3 >/dev/null 2>&1 \
    || loud_skip "MISSING PYTHON3" "python3 is required for graph assertions"

unset TMUX WG_DIR WG_TASK_ID WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER
scratch=$(make_scratch)
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
export TMUX_TMPDIR="$scratch/tmux"
mkdir -p "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR" "$TMUX_TMPDIR"
WG_BIN=$(command -v wg)
provider_log="$scratch/provider.argv"
fakebin="$scratch/fakebin"
mkdir -p "$fakebin"
for provider in claude codex pi opencode octomind dexto; do
    cat >"$fakebin/$provider" <<'SH'
#!/usr/bin/env bash
printf '%s' "$(basename "$0")" >>"$WG_PROVIDER_ARGV_LOG"
printf ' %q' "$@" >>"$WG_PROVIDER_ARGV_LOG"
printf '\n' >>"$WG_PROVIDER_ARGV_LOG"
while IFS= read -r line; do printf 'FAKE_PROVIDER_ECHO:%s\n' "$line"; done
SH
    chmod +x "$fakebin/$provider"
done
export PATH="$fakebin:$PATH"
export WG_PROVIDER_ARGV_LOG="$provider_log"
: >"$provider_log"

sessions_before=$(tmux list-sessions -F '#S' 2>/dev/null | sort || true)
active_sessions=()
cleanup_sessions() {
    for session in "${active_sessions[@]:-}"; do
        [[ -n "$session" ]] && tmux kill-session -t "$session" 2>/dev/null || true
    done
    for dir in "$scratch"/empty-*/.wg "$scratch"/terminal/.wg; do
        [[ -d "$dir" ]] && "$WG_BIN" --dir "$dir" service stop >/dev/null 2>&1 || true
    done
}
add_cleanup_hook cleanup_sessions

chat_count() {
    python3 - "$1/graph.jsonl" <<'PY'
import json, sys
p=sys.argv[1]
count=0
for line in open(p, encoding="utf-8"):
    if not line.strip(): continue
    row=json.loads(line)
    count += row.get("id", "").startswith(".chat-")
print(count)
PY
}

wait_screen() {
    local session="$1" needle="$2"
    for _ in $(seq 1 100); do
        tmux capture-pane -p -t "$session" 2>/dev/null | grep -qF "$needle" && return 0
        sleep 0.05
    done
    return 1
}

assert_no_bootstrap_mutation() {
    local g="$1" graph_hash="$2" label="$3"
    [[ "$(sha256sum "$g/graph.jsonl")" == "$graph_hash" ]] \
        || loud_fail "$label: graph changed during TUI open"
    [[ "$(chat_count "$g")" == 0 ]] \
        || loud_fail "$label: implicit .chat-N row appeared"
    [[ ! -e "$g/sessions.json" ]] \
        || loud_fail "$label: opening wrote session aliases: $(cat "$g/sessions.json")"
    [[ ! -d "$g/chat" ]] \
        || loud_fail "$label: opening wrote chat/UUID state: $(find "$g/chat" -maxdepth 3 -print)"
    [[ ! -e "$g/tui-state.json" ]] \
        || loud_fail "$label: empty TUI exit wrote synthetic selection state"
    [[ ! -e "$g/chat-history-0.jsonl" ]] \
        || loud_fail "$label: empty TUI exit wrote synthetic chat-zero history"
    [[ ! -s "$provider_log" ]] \
        || loud_fail "$label: provider CLI launched during open: $(cat "$provider_log")"
}

launch_empty_once() {
    local g="$1" session="$2" extra_env="${3:-}"
    local graph_hash
    graph_hash=$(sha256sum "$g/graph.jsonl")
    active_sessions+=("$session")
    tmux new-session -d -s "$session" -x 140 -y 42 \
        "env PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TMUX_TMPDIR='$TMUX_TMPDIR' WG_PROVIDER_ARGV_LOG='$provider_log' $extra_env '$WG_BIN' --dir '$g' tui"
    wait_screen "$session" "No chat selected" \
        || loud_fail "$session: empty TUI did not render No chat selected: $(tmux capture-pane -p -t "$session" 2>/dev/null | tail -20)"
    tmux capture-pane -p -t "$session" | grep -qF "New chat" \
        || loud_fail "$session: discoverable New chat control missing"
    assert_no_bootstrap_mutation "$g" "$graph_hash" "$session-before-quit"
    tmux send-keys -t "$session" q
    for _ in $(seq 1 30); do
        tmux has-session -t "$session" 2>/dev/null || break
        sleep 0.05
    done
    tmux kill-session -t "$session" 2>/dev/null || true
    assert_no_bootstrap_mutation "$g" "$graph_hash" "$session-after-quit"
}

# Twenty installed-binary open/restart cycles. First five have no route metadata,
# next five carry a syntactically valid but unusable route, and all stay provider-free.
root="$scratch/empty-main"
g="$root/.wg"
mkdir -p "$root"
"$WG_BIN" --dir "$g" init --no-agency >/dev/null 2>&1 \
    || loud_fail "empty graph init failed"
rm -f "$g/config.toml"
for n in $(seq 1 5); do launch_empty_once "$g" "wg-empty-missing-$$-$n"; done
cat >"$g/config.toml" <<'TOML'
[dispatcher]
model = "pi:"
TOML
for n in $(seq 6 10); do launch_empty_once "$g" "wg-empty-corrupt-$$-$n"; done

# Delayed full bootstrap: the prioritized empty-chat lane still renders without
# creating anything while the unrelated storage lane is blocked/delayed.
stall="$scratch/bootstrap-stall"
mkfifo "$stall"
( exec 9>"$stall"; sleep 10 ) &
stall_writer=$!
launch_empty_once "$g" "wg-empty-delayed-$$" "WG_TUI_TEST_STORAGE_STALL_PATH='$stall' WG_TUI_TEST_STORAGE_LATENCY_MS=1000"
rm -f "$stall"
kill "$stall_writer" 2>/dev/null || true
wait "$stall_writer" 2>/dev/null || true

# A live daemon changes no bootstrap semantics. Use a valid selected route only
# to start the control plane; the fake Pi argv log must remain empty.
cat >"$g/config.toml" <<'TOML'
[dispatcher]
model = "pi:openai-codex:gpt-5.6-sol"
TOML
"$WG_BIN" --dir "$g" service start >/dev/null 2>&1 \
    || loud_fail "daemon failed to start for daemon-up empty test"
for n in $(seq 11 20); do launch_empty_once "$g" "wg-empty-daemon-$$-$n"; done
"$WG_BIN" --dir "$g" service stop >/dev/null 2>&1 || true

# A live row with corrupt atomic route metadata must fail closed before any
# session preparation, tmux ownership, or fallback provider launch.
invalid_root="$scratch/invalid-route"
invalid_g="$invalid_root/.wg"
mkdir -p "$invalid_root"
"$WG_BIN" --dir "$invalid_g" init --no-agency >/dev/null 2>&1 || loud_fail "invalid-route init failed"
cat >"$invalid_g/config.toml" <<'TOML'
[dispatcher]
model = "pi:openai-codex:gpt-5.6-sol"
TOML
"$WG_BIN" --dir "$invalid_g" chat create --name invalid --exec pi --model pi:openai-codex:gpt-5.6-sol >/dev/null 2>&1 \
    || loud_fail "invalid-route fixture create failed"
python3 - "$invalid_g/graph.jsonl" "$invalid_g/service/coordinator-state-0.json" <<'PY'
import json, sys
p, state_path=sys.argv[1:]
rows=[]
for line in open(p, encoding="utf-8"):
    row=json.loads(line)
    if row.get("id")==".chat-0": row["model"]="pi:"
    rows.append(row)
with open(p,"w",encoding="utf-8") as f:
    for row in rows: f.write(json.dumps(row,separators=(",",":"))+"\n")
# Malformed atomic state used to be quarantined/renamed by ordinary runtime
# loaders. TUI bootstrap must inspect it read-only and leave the exact bytes.
open(state_path,"w",encoding="utf-8").write("{broken-route-state")
PY
invalid_graph_hash=$(sha256sum "$invalid_g/graph.jsonl")
invalid_state_hash=$(sha256sum "$invalid_g/service/coordinator-state-0.json")
invalid_sessions_hash=$(sha256sum "$invalid_g/sessions.json" 2>/dev/null || true)
invalid_session="wg-invalid-route-$$"
active_sessions+=("$invalid_session")
tmux new-session -d -s "$invalid_session" -x 140 -y 42 \
    "env PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TMUX_TMPDIR='$TMUX_TMPDIR' WG_PROVIDER_ARGV_LOG='$provider_log' '$WG_BIN' --dir '$invalid_g' tui"
wait_screen "$invalid_session" "corrupt saved route metadata" \
    || loud_fail "invalid route did not surface fail-closed recovery UI: $(tmux capture-pane -p -t "$invalid_session" 2>/dev/null | tail -20)"
tmux capture-pane -p -t "$invalid_session" | grep -qF "New chat" \
    || loud_fail "invalid route recovery UI lost explicit New chat control"
tmux send-keys -t "$invalid_session" q
sleep 0.2
tmux kill-session -t "$invalid_session" 2>/dev/null || true
[[ "$(sha256sum "$invalid_g/graph.jsonl")" == "$invalid_graph_hash" ]] || loud_fail "invalid route open mutated graph"
[[ "$(sha256sum "$invalid_g/service/coordinator-state-0.json")" == "$invalid_state_hash" ]] || loud_fail "invalid route open rewrote atomic route"
[[ "$(sha256sum "$invalid_g/sessions.json" 2>/dev/null || true)" == "$invalid_sessions_hash" ]] || loud_fail "invalid route open rewrote session aliases"
[[ ! -d "$invalid_g/chat" ]] || loud_fail "invalid route open prepared chat/UUID state"
[[ ! -e "$invalid_g/tui-state.json" && ! -e "$invalid_g/chat-history-0.jsonl" ]] \
    || loud_fail "invalid route open persisted synthetic chat selection/history"
[[ ! -s "$provider_log" ]] || loud_fail "invalid route launched provider fallback: $(cat "$provider_log")"

# Terminal-only graph: explicit CLI lifecycle establishes an archived chat,
# then opening the TUI must neither replace nor resurrect it.
terminal_root="$scratch/terminal"
terminal_g="$terminal_root/.wg"
mkdir -p "$terminal_root"
"$WG_BIN" --dir "$terminal_g" init --no-agency >/dev/null 2>&1 || loud_fail "terminal init failed"
cat >"$terminal_g/config.toml" <<'TOML'
[dispatcher]
model = "pi:openai-codex:gpt-5.6-sol"
TOML
"$WG_BIN" --dir "$terminal_g" chat create --name terminal --command /bin/true >/dev/null 2>&1 \
    || loud_fail "terminal fixture chat create failed"
"$WG_BIN" --dir "$terminal_g" chat archive 0 >/dev/null 2>&1 \
    || loud_fail "terminal fixture archive failed"
terminal_hash=$(sha256sum "$terminal_g/graph.jsonl")
terminal_count=$(chat_count "$terminal_g")
[[ "$terminal_count" == 1 ]] || loud_fail "terminal fixture missing archived row"
terminal_session="wg-terminal-only-$$"
active_sessions+=("$terminal_session")
tmux new-session -d -s "$terminal_session" -x 140 -y 42 \
    "env PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TMUX_TMPDIR='$TMUX_TMPDIR' WG_PROVIDER_ARGV_LOG='$provider_log' '$WG_BIN' --dir '$terminal_g' tui"
wait_screen "$terminal_session" "No chat selected" \
    || loud_fail "terminal-only graph did not render No chat selected"
[[ "$(sha256sum "$terminal_g/graph.jsonl")" == "$terminal_hash" ]] \
    || loud_fail "terminal-only open mutated archived row"
[[ "$(chat_count "$terminal_g")" == 1 ]] \
    || loud_fail "terminal-only open resurrected/replaced archived chat"
tmux send-keys -t "$terminal_session" q
tmux kill-session -t "$terminal_session" 2>/dev/null || true

create_via_launcher() {
    local mode="$1" ordinal="$2"
    local root="$scratch/explicit-$mode" g="$scratch/explicit-$mode/.wg"
    mkdir -p "$root"
    "$WG_BIN" --dir "$g" init --no-agency >/dev/null 2>&1 || loud_fail "$mode init failed"
    local session="wg-explicit-$mode-$$"
    active_sessions+=("$session")
    tmux new-session -d -s "$session" -x 140 -y 42 \
        "env PATH='$PATH' HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' TMUX_TMPDIR='$TMUX_TMPDIR' WG_PROVIDER_ARGV_LOG='$provider_log' '$WG_BIN' --dir '$g' tui"
    wait_screen "$session" "No chat selected" || loud_fail "$mode: no empty state"
    if [[ "$mode" == keyboard ]]; then
        tmux send-keys -t "$session" n
    else
        local xy
        xy=$(tmux capture-pane -p -t "$session" | python3 -c '
import sys
rows=sys.stdin.read().splitlines()
ys=[i for i,row in enumerate(rows) if "New chat" in row]
if not ys: raise SystemExit(1)
y=ys[-1]; x=rows[y].index("New chat")
print(x+2, y+1)
') || loud_fail "pointer: could not locate New chat control"
        local x=${xy% *} y=${xy#* }
        tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sM' "$x" "$y")"
        tmux send-keys -t "$session" -l "$(printf '\033[<0;%s;%sm' "$x" "$y")"
    fi
    wait_screen "$session" "Add new" || wait_screen "$session" "Claude Opus" \
        || loud_fail "$mode: explicit control did not open route chooser: $(tmux capture-pane -p -t "$session" 2>/dev/null | tail -20)"
    tmux send-keys -t "$session" Enter
    for _ in $(seq 1 100); do
        [[ "$(chat_count "$g")" == 1 ]] && break
        sleep 0.05
    done
    [[ "$(chat_count "$g")" == 1 ]] \
        || loud_fail "$mode: confirmation did not create exactly one chat"
    python3 - "$g/graph.jsonl" "$ordinal" <<'PY'
import json, sys
rows=[json.loads(line) for line in open(sys.argv[1], encoding="utf-8") if json.loads(line).get("id", "").startswith(".chat-")]
assert len(rows)==1, rows
row=rows[0]
assert row.get("id")==".chat-0", row
assert row.get("executor_preset_name") in {"codex", "claude"}, row
assert row.get("model"), row
assert row.get("command_argv"), row
PY
    sleep 0.3
    [[ "$(chat_count "$g")" == 1 ]] || loud_fail "$mode: duplicate chat appeared after confirmation"
    tmux kill-session -t "$session" 2>/dev/null || true
}

create_via_launcher keyboard 1
create_via_launcher pointer 2

sessions_after=$(tmux list-sessions -F '#S' 2>/dev/null | sort || true)
# Ignore explicit launcher panes (cleaned by the hook); the twenty open-only
# rounds must not leave a project chat tmux session behind.
if comm -13 <(printf '%s\n' "$sessions_before") <(printf '%s\n' "$sessions_after") | grep -q 'wg-chat-empty'; then
    loud_fail "open-only rounds left a handler tmux session behind"
fi

echo "PASS: 20 empty TUI opens (missing/corrupt route, daemon down/up, delayed bootstrap) were graph/session/provider-free; terminal-only graph stayed terminal; explicit n and pointer controls each created exactly one pinned chat"
