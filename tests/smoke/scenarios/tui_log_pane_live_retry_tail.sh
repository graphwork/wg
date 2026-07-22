#!/usr/bin/env bash
# Scenario: tui_log_pane_live_retry_tail
#
# Human-flow regression for fix-live-attempt-log-tail. A failed agent-620 is
# followed by assigned/live agent-623 whose Pi raw stream is a sparse ~600 MB
# file with an enormous early record. The real TUI must open attempt 2 at EOF,
# follow a simulated long-running `cargo test --bin wg` completion, cycle to
# attempt 1 with `{`, and return to the current live tail with `}`.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required for the live TUI flow"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required for the sparse fixture"

scratch=$(make_scratch)
session="wgsmoke-live-retry-tail-$$"
writer_pid=""
cleanup_live_retry_tail() {
    [[ -z "$writer_pid" ]] || kill "$writer_pid" 2>/dev/null || true
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook cleanup_live_retry_tail

cd "$scratch"
wg init -x claude >init.log 2>&1 || loud_fail "wg init failed: $(tail -5 init.log)"
wg add "Retried Pi task" --id live-retry-tail >add.log 2>&1 \
    || loud_fail "wg add failed: $(tail -5 add.log)"
graph_dir="$scratch/.wg"

python3 - "$graph_dir/graph.jsonl" <<'PY'
import json, sys
path = sys.argv[1]
out = []
for line in open(path):
    if not line.strip():
        continue
    obj = json.loads(line)
    if obj.get("kind") == "task" and obj.get("id") == "live-retry-tail":
        obj["status"] = "in-progress"
        obj["assigned"] = "agent-623"
        obj["retry_count"] = 1
        obj["log"] = [
            {"timestamp": "2026-07-22T15:00:00Z", "actor": "agent-620",
             "message": "Claimed by autonomous agent"},
            {"timestamp": "2026-07-22T15:00:30Z", "actor": "agent-620",
             "message": "Agent terminated after ENOSPC"},
            {"timestamp": "2026-07-22T15:01:00Z",
             "message": "Task reset for retry from in-progress (attempt #1)"},
            {"timestamp": "2026-07-22T15:01:30Z", "actor": "agent-623",
             "message": "Claimed by autonomous agent"},
        ]
    out.append(json.dumps(obj))
open(path, "w").write("\n".join(out) + "\n")
PY

mkdir -p "$graph_dir/service" "$graph_dir/agents/agent-620" "$graph_dir/agents/agent-623"
python3 - "$graph_dir/service/registry.json" "$$" <<'PY'
import json, sys
path, pid = sys.argv[1], int(sys.argv[2])
reg = {"next_agent_id": 624, "agents": {
    "agent-620": {
        "id": "agent-620", "pid": pid, "task_id": "live-retry-tail",
        "executor": "pi", "started_at": "2026-07-22T15:00:00Z",
        "last_heartbeat": "2026-07-22T15:00:30Z", "status": "failed",
        "output_file": ".wg/agents/agent-620/output.log",
        "completed_at": "2026-07-22T15:00:30Z"
    },
    "agent-623": {
        "id": "agent-623", "pid": pid, "task_id": "live-retry-tail",
        "executor": "pi", "started_at": "2026-07-22T15:01:30Z",
        "last_heartbeat": "2026-07-22T15:02:00Z", "status": "working",
        "output_file": ".wg/agents/agent-623/output.log"
    }
}}
open(path, "w").write(json.dumps(reg, indent=2))
PY

failed_marker="FAILED_AGENT_620_ONLY_$$"
early_marker="ATTEMPT_2_STALE_STARTUP_$$"
completion_marker="CARGO_TEST_COMPLETED_$$"
printf '{"type":"turn_end","message":{"content":[{"type":"text","text":"%s"}]}}\n' "$failed_marker" \
    >"$graph_dir/agents/agent-620/raw_stream.jsonl"
: >"$graph_dir/agents/agent-620/output.log"
: >"$graph_dir/agents/agent-623/output.log"

# Sparse 600 MB stream: one enormous prefix record, then >200 complete Pi
# events, then the current command. A prefix reader cannot reach the command;
# one reverse-tail page can.
python3 - "$graph_dir/agents/agent-623/raw_stream.jsonl" "$early_marker" <<'PY'
import json, os, sys
path, early = sys.argv[1:]
size = 600 * 1024 * 1024
with open(path, "wb") as f:
    f.truncate(size)
    f.seek(size - 128 * 1024)
    f.write(b"}\n")  # terminate the enormous early record
    f.write((json.dumps({"type": "turn_end", "message": {"content": [
        {"type": "text", "text": early}
    ]}}) + "\n").encode())
    for i in range(260):
        f.write((json.dumps({"type": "turn_end", "message": {"content": [
            {"type": "text", "text": f"bounded filler {i:03}"}
        ]}}) + "\n").encode())
    f.write((json.dumps({"type": "tool_execution_start", "toolName": "bash",
                         "args": {"command": "cargo test --bin wg"}}) + "\n").encode())
    f.truncate(f.tell())
PY

# Simulate completion of the slow operation after the first live-tail dump.
(
    sleep 9
    python3 - "$graph_dir/agents/agent-623/raw_stream.jsonl" "$completion_marker" <<'PY'
import json, sys
path, marker = sys.argv[1:]
with open(path, "a") as f:
    f.write(json.dumps({"type": "tool_execution_end", "isError": False,
                        "result": {"content": [{"text": marker}]}}) + "\n")
PY
) &
writer_pid=$!

tmux new-session -d -s "$session" -x 220 -y 60 \
    "cd $scratch && env -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER WG_USER=unknown wg tui"
sleep 4
# This isolated graph has no live chat PTY, so startup is already in command
# mode. Drive the exact documented '4' Log-tab key directly.
tmux send-keys -t "$session" 4
sleep 2

dump_screen() {
    local out="$1"
    (cd "$scratch" && wg tui-dump >"$out" 2>&1) || loud_fail "tui-dump failed: $(cat "$out")"
}

live_dump="$scratch/live.txt"
dump_screen "$live_dump"
grep -q "attempt 2/2 (live)" "$live_dump" \
    || loud_fail "header did not identify live attempt 2/2:\n$(cat "$live_dump")"
grep -q "agent=agent-623 src=now" "$live_dump" \
    || loud_fail "header/body source was not coherent for agent-623:\n$(cat "$live_dump")"
grep -q "cargo test --bin wg" "$live_dump" \
    || loud_fail "first bounded request did not show the current cargo test operation:\n$(cat "$live_dump")"
! grep -q "$failed_marker" "$live_dump" \
    || loud_fail "live attempt was overwritten by failed agent-620:\n$(cat "$live_dump")"
! grep -q "$early_marker" "$live_dump" \
    || loud_fail "live pane opened on stale attempt-2 startup instead of its tail:\n$(cat "$live_dump")"

# The same-source cursor must follow the append and render completion once.
# Poll the rendered surface (not the file) so slower CI scheduling is harmless.
complete_dump="$scratch/complete.txt"
for _ in $(seq 1 15); do
    sleep 1
    dump_screen "$complete_dump"
    grep -Fq "✓⌁bash → cargo test --bin wg" "$complete_dump" && break
done
grep -Fq "✓⌁bash → cargo test --bin wg" "$complete_dump" \
    || loud_fail "live attempt did not incrementally follow command completion:\n$(cat "$complete_dump")"

# Events view intentionally folds the result into the call's ✓ status. Cycle
# through high-level to raw with the same real '4' key twice; raw exposes the
# exact completion payload and lets us prove it arrived exactly once.
tmux send-keys -t "$session" 4
sleep 0.3
tmux send-keys -t "$session" 4
sleep 1
dump_screen "$complete_dump"
grep -q "$completion_marker" "$complete_dump" \
    || loud_fail "raw view did not expose command completion payload:\n$(cat "$complete_dump")"
[[ $(grep -o "$completion_marker" "$complete_dump" | wc -l) -eq 1 ]] \
    || loud_fail "completion event was duplicated after refresh:\n$(cat "$complete_dump")"

# Exact human keys: previous attempt, then back to current live tail.
tmux send-keys -t "$session" -l '{'
sleep 2
failed_dump="$scratch/failed.txt"
dump_screen "$failed_dump"
grep -q "attempt 1/2 (failed)" "$failed_dump" \
    || loud_fail "'{' did not select failed attempt 1/2:\n$(cat "$failed_dump")"
grep -q "agent=agent-620 src=now" "$failed_dump" \
    || loud_fail "failed attempt header/body source mismatch:\n$(cat "$failed_dump")"
grep -q "$failed_marker" "$failed_dump" \
    || loud_fail "failed attempt did not show its own bounded log:\n$(cat "$failed_dump")"

tmux send-keys -t "$session" -l '}'
sleep 2
return_dump="$scratch/return.txt"
dump_screen "$return_dump"
grep -q "attempt 2/2 (live)" "$return_dump" \
    || loud_fail "'}' did not return to live attempt 2/2:\n$(cat "$return_dump")"
grep -q "agent=agent-623 src=now" "$return_dump" \
    || loud_fail "returned live source was not agent-623/current:\n$(cat "$return_dump")"
grep -q "$completion_marker" "$return_dump" \
    || loud_fail "returning live landed on startup instead of current completion:\n$(cat "$return_dump")"

echo "PASS: live retry Session Log reverse-tails 600 MB Pi stream, follows completion, and cycles attempts"
