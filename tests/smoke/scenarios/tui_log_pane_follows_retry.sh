#!/usr/bin/env bash
# Scenario: tui_log_pane_follows_retry
#
# Regression (fix-tui-retry-log): after a fail→retry, the per-task Log pane
# stuck on the FIRST (failed) attempt's agent because load_log_pane() picked the
# first `agent-*` mentioned in the task log. The dispatcher had moved on to a
# live retry agent, but the pane kept surfacing the dead attempt's events — the
# task looked "stuck on the old log" even though the graph was progressing.
#
# This scenario reproduces the exact shape and drives the real human flow:
#   1. Boots a .wg layout with ONE in-progress task whose log mentions the
#      failed attempt (agent-fail) FIRST and the live retry (agent-live) second,
#      with `assigned` = agent-live. Both agents have a raw_stream.jsonl with a
#      distinct marker; the registry marks agent-fail Failed and agent-live
#      Working so the liveness label renders.
#   2. Launches `wg tui` in tmux, switches to the Log tab ('4').
#   3. Asserts (via `wg tui-dump`) the pane shows the LIVE agent's marker +
#      "attempt 2/2 (live)" + agent=agent-live BY DEFAULT — NOT the failed
#      attempt's marker.
#   4. Presses '{' (previous attempt) and asserts the pane switches to the
#      failed attempt: its marker + "attempt 1/2 (failed)".
#
# Requires: tmux, python3, wg on PATH.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive interactive TUI"
fi
if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 needed to build the synthetic .wg layout"
fi

scratch=$(make_scratch)
session="wgsmoke-tuiretry-$$"
kill_tmux_session() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook kill_tmux_session

cd "$scratch"

if ! wg init -x claude >init.log 2>&1; then
    loud_fail "wg init failed during smoke setup: $(tail -5 init.log)"
fi

graph_dir="$scratch/.wg"
if [[ ! -f "$graph_dir/graph.jsonl" ]]; then
    loud_fail "could not locate graph.jsonl under .wg/ after init"
fi

if ! wg add "Retried task" --id smoke-retry >add.log 2>&1; then
    loud_fail "wg add failed during smoke setup: $(tail -5 add.log)"
fi

fail_marker="WG_RETRY_SMOKE_FAILED_$$"
live_marker="WG_RETRY_SMOKE_LIVE_$$"

# Mark in-progress + assigned to the LIVE agent, and write a retry-shaped log
# that mentions the FAILED agent first (the trap the old code fell into).
python3 - "$graph_dir/graph.jsonl" <<'PY'
import json, sys
path = sys.argv[1]
out = []
for line in open(path):
    if not line.strip():
        continue
    obj = json.loads(line)
    if obj.get("kind") == "task" and obj.get("id") == "smoke-retry":
        obj["status"] = "in-progress"
        obj["assigned"] = "agent-live"
        obj["retry_count"] = 1
        obj["log"] = [
            {"timestamp": "2026-06-24T00:00:00Z", "actor": "agent-fail",
             "message": "Claimed by autonomous agent"},
            {"timestamp": "2026-06-24T00:00:30Z", "actor": "agent-fail",
             "message": "[wrapper] Agent exited with code 1, marking task failed"},
            {"timestamp": "2026-06-24T00:01:00Z",
             "message": "Task reset for retry from in-progress (attempt #1)"},
            {"timestamp": "2026-06-24T00:01:30Z", "actor": "agent-live",
             "message": "Claimed by autonomous agent"},
        ]
    out.append(json.dumps(obj))
open(path, "w").write("\n".join(out) + "\n")
PY

# Registry: agent-fail = Failed, agent-live = Working (drives the liveness label).
mkdir -p "$graph_dir/service"
python3 - "$graph_dir/service/registry.json" "$$" <<'PY'
import json, sys
path, pid = sys.argv[1], int(sys.argv[2])
reg = {
    "next_agent_id": 281,
    "agents": {
        "agent-fail": {
            "id": "agent-fail", "pid": pid, "task_id": "smoke-retry",
            "executor": "claude", "started_at": "2026-06-24T00:00:00Z",
            "last_heartbeat": "2026-06-24T00:00:45Z", "status": "failed",
            "output_file": ".wg/agents/agent-fail/output.log",
            "completed_at": "2026-06-24T00:00:45Z",
        },
        "agent-live": {
            "id": "agent-live", "pid": pid, "task_id": "smoke-retry",
            "executor": "claude", "started_at": "2026-06-24T00:01:30Z",
            "last_heartbeat": "2026-06-24T00:01:35Z", "status": "working",
            "output_file": ".wg/agents/agent-live/output.log",
        },
    },
}
open(path, "w").write(json.dumps(reg, indent=2))
PY

# raw_stream.jsonl for each agent, each carrying a distinct text marker.
for pair in "agent-fail:$fail_marker" "agent-live:$live_marker"; do
    aid="${pair%%:*}"
    marker="${pair##*:}"
    mkdir -p "$graph_dir/agents/$aid"
    cat >"$graph_dir/agents/$aid/raw_stream.jsonl" <<EOF
{"type":"system","subtype":"init","cwd":"$scratch","session_id":"$aid","tools":["Bash"]}
{"type":"assistant","message":{"content":[{"type":"text","text":"$marker"}]}}
EOF
    : >"$graph_dir/agents/$aid/output.log"
done

# Launch wg tui in tmux. Wide window so the Log header fits on one line.
tmux new-session -d -s "$session" -x 220 -y 60 "cd $scratch && wg tui"
sleep 4

# Esc out of the chat PTY focus, then '4' switches the right panel to Log.
tmux send-keys -t "$session" 'Escape'
sleep 1
tmux send-keys -t "$session" '4'
sleep 3

dump1="$scratch/dump1.txt"
if ! ( cd "$scratch" && wg tui-dump >"$dump1" 2>&1 ); then
    loud_fail "wg tui-dump failed:\n$(cat "$dump1")"
fi

# DEFAULT must follow the LIVE agent, not the failed first attempt.
if grep -q "$fail_marker" "$dump1"; then
    loud_fail "Log pane shows the FAILED attempt's marker by default after retry (the bug).\nDump:\n$(cat "$dump1")"
fi
if ! grep -q "$live_marker" "$dump1"; then
    loud_fail "Log pane did not render the LIVE agent's marker by default.\nDump:\n$(cat "$dump1")"
fi
if ! grep -q "agent=agent-live" "$dump1"; then
    loud_fail "Log header should report agent=agent-live by default.\nDump:\n$(cat "$dump1")"
fi
if ! grep -q "attempt 2/2" "$dump1"; then
    loud_fail "Log header should label the displayed attempt as 'attempt 2/2'.\nDump:\n$(cat "$dump1")"
fi

# Manual switcher: '{' goes to the previous (failed) attempt.
tmux send-keys -t "$session" -l '{'
sleep 2

dump2="$scratch/dump2.txt"
( cd "$scratch" && wg tui-dump >"$dump2" 2>&1 ) || true

if ! grep -q "$fail_marker" "$dump2"; then
    loud_fail "After '{' the Log pane should show the FAILED attempt's marker.\nDump:\n$(cat "$dump2")"
fi
if ! grep -q "attempt 1/2" "$dump2"; then
    loud_fail "After '{' the Log header should label the displayed attempt as 'attempt 1/2'.\nDump:\n$(cat "$dump2")"
fi

echo "PASS: Log pane defaults to the live retry agent and '{' cycles to the failed attempt"
exit 0
