#!/usr/bin/env bash
# Scenario: tui_log_pane_renders_raw_stream
#
# Regression: the Log pane (right-panel tab '4') showed
# "(no agent output yet — is the task running?)" even when an in-progress
# task's assigned agent had a populated raw_stream.jsonl. The original
# tui-agent-activity work added the parsing + rendering machinery but the
# render-time lazy-load path forgot to call update_log_stream_events()
# alongside load_log_pane() and update_log_output(). Result: stream events
# only refreshed on the slow 1s tick / fs-change debounce, NOT on the first
# draw of the Log tab. To the user, the tab looked permanently broken.
#
# This scenario:
#   1. Boots a synthetic .wg layout with one in-progress task assigned to
#      a fake agent whose raw_stream.jsonl already contains JSONL events.
#   2. Launches `wg tui` inside tmux, lets it draw, sends '4' to switch
#      to the Log tab.
#   3. Uses `wg tui-dump` to read the rendered cell grid back out.
#   4. Asserts the dump (a) does NOT contain the broken-state sentinel
#      and (b) DOES contain a unique marker we placed in the stream file.
#
# Requires: tmux, python3, wg on PATH.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
    WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
else
    require_wg
    WG_BIN="$(command -v wg)"
fi
[[ -x "$WG_BIN" ]] || loud_fail "wg binary is not executable: $WG_BIN"

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive interactive TUI"
fi
if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 needed to mutate graph.jsonl"
fi

scratch=$(make_scratch)
session="wgsmoke-tuilog-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session

# Treat CWD discovery and inherited worker/user state as hostile. The scratch
# may itself be nested below a live graph, so every WG process receives the
# exact fixture graph and a scratch-owned user/config home.
project="$scratch/project"
graph_dir="$project/.wg"
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export WG_GLOBAL_DIR="$HOME/.wg"
unset TMUX TMUX_TMPDIR WG_DIR WG_PROJECT_ROOT WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH
unset WG_TASK_ID WG_AGENT_ID WG_SPAWN_EPOCH WG_EXECUTOR_TYPE WG_MODEL WG_TIER
mkdir -p "$project" "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR"

init_log="$scratch/init.log"
if ! "$WG_BIN" --dir "$graph_dir" init --no-agency >"$init_log" 2>&1; then
    loud_fail "wg init failed during smoke setup: $(tail -5 "$init_log")"
fi
if [[ ! -f "$graph_dir/graph.jsonl" ]]; then
    loud_fail "could not locate the explicitly initialized graph at $graph_dir"
fi

add_log="$scratch/add.log"
if ! "$WG_BIN" --dir "$graph_dir" add "Live agent task" --id smoke-live >"$add_log" 2>&1; then
    loud_fail "wg add failed during smoke setup: $(tail -5 "$add_log")"
fi

# Mark the task in-progress and assigned to agent-fake.
python3 - "$graph_dir/graph.jsonl" <<'PY'
import json, sys
path = sys.argv[1]
out = []
for line in open(path):
    if not line.strip():
        continue
    obj = json.loads(line)
    if obj.get("kind") == "task" and obj.get("id") == "smoke-live":
        obj["status"] = "in-progress"
        obj["assigned"] = "agent-fake"
    out.append(json.dumps(obj))
open(path, "w").write("\n".join(out) + "\n")
PY

# Place a raw_stream.jsonl that mimics the format claude-handler writes.
mkdir -p "$graph_dir/agents/agent-fake"
marker="WG_TUI_LOG_SMOKE_MARKER_$$"
cat >"$graph_dir/agents/agent-fake/raw_stream.jsonl" <<EOF
{"type":"system","subtype":"init","cwd":"$scratch","session_id":"smoke","tools":["Bash"]}
{"type":"assistant","message":{"content":[{"type":"text","text":"$marker"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"echo from-smoke"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"from-smoke","is_error":false}]}}
EOF
: >"$graph_dir/agents/agent-fake/output.log"

# Launch wg tui in tmux. Wide window so the Log pane has room.
tui_err="$scratch/tui.err"
tmux new-session -d -s "$session" -x 200 -y 60 \
    "cd '$project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_USER=unknown '$WG_BIN' --dir '$graph_dir' tui 2>'$tui_err'; printf '%s\\n' \$? >'$scratch/tui.exit'"
sleep 4

# The isolated graph has no chat PTY, so startup is already in command mode.
# '4' switches the right panel to Log without an Escape that would quit TUI.
tmux send-keys -t "$session" '4'
sleep 3

# Pull the rendered screen back out via the dump server.
dump_out="$scratch/dump.txt"
if ! "$WG_BIN" --dir "$graph_dir" tui-dump >"$dump_out" 2>&1; then
    loud_fail "wg tui-dump failed:\n$(cat "$dump_out")\nTUI exit: $(cat "$scratch/tui.exit" 2>/dev/null || echo running)\nTUI screen:\n$(tmux capture-pane -p -t "$session" 2>/dev/null)\nTUI stderr:\n$(cat "$tui_err" 2>/dev/null)"
fi

if grep -q "no agent output yet" "$dump_out"; then
    loud_fail "Log pane still shows 'no agent output yet' despite raw_stream.jsonl having events.\nDump:\n$(cat "$dump_out")"
fi

if ! grep -q "$marker" "$dump_out"; then
    loud_fail "Log pane did not render the unique stream marker '$marker'.\nDump:\n$(cat "$dump_out")"
fi

# Auto-refresh check: append a new event and verify it shows up on a
# subsequent dump (within a few ticks).
marker2="WG_TUI_LOG_SMOKE_NEW_$$"
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"%s"}]}}\n' "$marker2" \
    >>"$graph_dir/agents/agent-fake/raw_stream.jsonl"
dump_out2="$scratch/dump2.txt"
for _ in $(seq 1 15); do
    sleep 1
    "$WG_BIN" --dir "$graph_dir" tui-dump >"$dump_out2" 2>&1 || true
    grep -q "$marker2" "$dump_out2" && break
done

if ! grep -q "$marker2" "$dump_out2"; then
    loud_fail "Log pane did not pick up newly-appended stream event '$marker2' within 15s.\nDump:\n$(cat "$dump_out2")"
fi

echo "PASS: Log tab renders raw_stream.jsonl events and auto-refreshes"
exit 0
