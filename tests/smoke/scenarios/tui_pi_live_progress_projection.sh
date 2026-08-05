#!/usr/bin/env bash
# Real tmux TUI regression for cumulative Pi tool progress coalescing.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then
  WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
else
  require_wg
  WG_BIN="$(command -v wg)"
fi
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 required"

scratch=$(make_scratch)
project="$scratch/project"
G="$project/.wg"
HOME="$scratch/home"; export HOME
XDG_CONFIG_HOME="$HOME/.config"; export XDG_CONFIG_HOME
WG_GLOBAL_DIR="$HOME/.wg"; export WG_GLOBAL_DIR
mkdir -p "$project" "$HOME" "$XDG_CONFIG_HOME" "$WG_GLOBAL_DIR"
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_MODEL WG_EXECUTOR_TYPE WG_PROJECT_ROOT WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH
unset WG_WORKER_CAPABILITY WG_WORKER_CONTROL_PROTOCOL WG_WORKER_IPC
"$WG_BIN" --dir "$G" init --no-agency >/dev/null
"$WG_BIN" --dir "$G" add "Pi live progress" --id pi-live-progress >/dev/null
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
p=sys.argv[1]; rows=[]
for line in open(p):
    v=json.loads(line)
    if v.get("kind")=="task" and v.get("id")=="pi-live-progress":
        v["status"]="in-progress"; v["assigned"]="agent-pi-live"
    rows.append(json.dumps(v))
open(p,"w").write("\n".join(rows)+"\n")
PY
mkdir -p "$G/agents/agent-pi-live"
printf '%s\n' 'PI_STDERR_DIAGNOSTIC_ONLY' >"$G/agents/agent-pi-live/output.log"
python3 - "$G/agents/agent-pi-live/raw_stream.jsonl" <<'PY'
import json,sys
p=sys.argv[1]
with open(p,"w") as f:
    f.write(json.dumps({"type":"tool_execution_start","toolCallId":"build-1","toolName":"bash","args":{"command":"cargo test --workspace"}})+"\n")
    for i in range(260):
        f.write(json.dumps({"type":"tool_execution_update","toolCallId":"build-1","toolName":"bash","args":{"command":"cargo test --workspace"},"partialResult":{"content":[{"type":"text","text":f"tests completed {i}/260\nPI_LATEST_PROGRESS_{i}"}]}})+"\n")
    # Keep the transcript fixture inside the bounded reverse-tail window.
    f.write(json.dumps({"type":"turn_end","message":{"content":[
        {"type":"thinking","thinking":"PI_CHAT_STYLE_THINKING"},
        {"type":"text","text":"## PI_CHAT_STYLE_HEADING\n\n- **PI_MARKDOWN_BOLD**\n- `PI_MARKDOWN_CODE`"}
    ]}})+"\n")
PY

session="wgsmoke-pi-progress-$$"
cleanup_tmux() { tmux kill-session -t "$session" 2>/dev/null || true; }
add_cleanup_hook cleanup_tmux
tmux new-session -d -s "$session" -x 180 -y 50 \
  "cd '$project' && env HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_USER=unknown '$WG_BIN' --dir '$G' tui 2>'$scratch/tui.err'"
sleep 3
tmux send-keys -t "$session" 4

wait_dump() {
  local needle=$1 label=$2 out="$scratch/dump.txt"
  for _ in $(seq 1 20); do
    sleep .5
    "$WG_BIN" --dir "$G" tui-dump >"$out" 2>&1 || true
    grep -q "$needle" "$out" && return 0
  done
  loud_fail "$label; dump=$(cat "$out" 2>/dev/null); stderr=$(cat "$scratch/tui.err" 2>/dev/null)"
}
assert_clean_projection() {
  local mode=$1
  wait_dump "PI_LATEST_PROGRESS_259" "$mode blanked instead of showing latest Pi progress"
  grep -qi 'bash' "$scratch/dump.txt" || loud_fail "$mode omitted current tool: $(cat "$scratch/dump.txt")"
  ! grep -Eq 'PI_LATEST_PROGRESS_(1|2|3|4|5|6|7|8|9|10)[^0-9]' "$scratch/dump.txt" || \
    loud_fail "$mode rendered cumulative update spam: $(cat "$scratch/dump.txt")"
  ! grep -Eq 'partialResult|toolCallId|tool_execution_update' "$scratch/dump.txt" || \
    loud_fail "$mode leaked native JSON: $(cat "$scratch/dump.txt")"
}

# Pi Logs must open on the chat-style cleaned transcript, not on the optional
# Events/HighLevel projections. Markdown, thinking, and tools share that view.
wait_dump 'view=\[Pretty\]' 'Pi Log did not default to the chat-style Pretty transcript'
assert_clean_projection Pretty
for transcript_text in PI_CHAT_STYLE_THINKING PI_CHAT_STYLE_HEADING PI_MARKDOWN_BOLD PI_MARKDOWN_CODE; do
  grep -q "$transcript_text" "$scratch/dump.txt" || \
    loud_fail "Pretty omitted Pi chat transcript content $transcript_text: $(cat "$scratch/dump.txt")"
done

# Incremental cumulative replacement must update the same visible slot.
python3 - "$G/agents/agent-pi-live/raw_stream.jsonl" <<'PY'
import json,sys
with open(sys.argv[1],"a") as f:
    f.write(json.dumps({"type":"tool_execution_update","toolCallId":"build-1","toolName":"bash","args":{"command":"cargo test --workspace"},"partialResult":{"content":[{"type":"text","text":"all tests still running\nPI_LIVE_APPEND_FINAL"}]}})+"\n")
PY
wait_dump 'PI_LIVE_APPEND_FINAL' 'incremental Pi update did not replace live projection'
! grep -q 'PI_LATEST_PROGRESS_259' "$scratch/dump.txt" || loud_fail "old cumulative progress remained after replacement"

# End replaces the running projection with one named final result.
python3 - "$G/agents/agent-pi-live/raw_stream.jsonl" <<'PY'
import json,sys
with open(sys.argv[1],"a") as f:
    f.write(json.dumps({"type":"tool_execution_end","toolCallId":"build-1","toolName":"bash","result":{"content":[{"type":"text","text":"PI_FINAL_RESULT: 260 tests passed"}]},"isError":False})+"\n")
PY
wait_dump 'PI_FINAL_RESULT' 'Pi end did not replace running projection with final result'
! grep -q 'PI_LIVE_APPEND_FINAL' "$scratch/dump.txt" || loud_fail "running progress survived finalized replacement"
[[ $(grep -o 'PI_FINAL_RESULT' "$scratch/dump.txt" | wc -l) -eq 1 ]] || loud_fail "final result rendered more than once"

# Raw remains an explicit, byte-faithful diagnostic mode. Pretty -> Raw is one
# view cycle, and native JSON must reappear there unchanged.
tmux send-keys -t "$session" 4
wait_dump 'view=\[Raw\]' 'did not enter explicit Raw diagnostic mode'
wait_dump '"toolCallId": "build-1"' 'Raw did not preserve native Pi JSON bytes'
grep -q '"type": "tool_execution_end"' "$scratch/dump.txt" || loud_fail "Raw omitted Pi end record"

echo 'PASS: Pi Log defaults to chat-style markdown/thinking/tools with live in-place updates; Raw stays exact'
