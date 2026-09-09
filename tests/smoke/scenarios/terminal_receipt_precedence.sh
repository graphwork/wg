#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/_helpers.sh"
require_wg

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required to inspect terminal classifier JSON"
fi

scratch="$(make_scratch)"
completed="$scratch/completed.jsonl"
blocked="$scratch/blocked.jsonl"
provider="$scratch/provider.jsonl"
preterminal="$scratch/preterminal.jsonl"
ambiguous="$scratch/ambiguous.jsonl"

cat >"$completed" <<'JSONL'
{"type":"tool_execution_end","result":"acceptance discusses provider timeout and reset heuristics"}
{"type":"turn_end","message":{"role":"assistant","responseId":"done-1","stopReason":"stop","rawStopReason":"completed"}}
JSONL
cp "$completed" "$blocked"
printf '%s\n' '{"type":"finalization_blocked","code":"completion_needs_review"}' >>"$blocked"
printf '%s\n' '{"type":"error","error":{"code":408,"message":"request timed out","metadata":{"error_type":"timeout"}}}' >"$provider"
printf '%s\n' '{"type":"turn_end","message":{"responseId":"tool-1","stopReason":"toolUse","rawStopReason":"completed"}}' >"$preterminal"
cat >"$ambiguous" <<'JSONL'
{"type":"turn_end","message":{"responseId":"done-1","stopReason":"completed"}}
{"type":"agent_end","messages":[{"role":"assistant","responseId":"failed-2","stopReason":"failed"}]}
JSONL

classify() {
    local file="$1" exit_code="$2"
    wg classify-failure --terminal --json --executor pi \
        --raw-stream "$file" --exit-code "$exit_code"
}

completed_first="$(classify "$completed" 124)"
completed_replay="$(classify "$completed" 124)"
[[ "$completed_first" == "$completed_replay" ]] || loud_fail "terminal replay was not byte-stable"

python3 - "$completed_first" "$blocked" "$provider" "$preterminal" "$ambiguous" <<'PY'
import json, subprocess, sys
completed = json.loads(sys.argv[1])
assert completed["state"] == "completed", completed
assert "failure_reason" not in completed, completed

cases = [
    (sys.argv[2], 1, "finalization-blocked", None, "needs-review"),
    (sys.argv[3], 1, "provider-failure", "timeout", None),
    (sys.argv[4], 124, "provider-failure", "hard-timeout", None),
    (sys.argv[5], 1, "ambiguous", None, None),
]
for path, code, state, failure, blocker in cases:
    out = subprocess.check_output([
        "wg", "classify-failure", "--terminal", "--json", "--executor", "pi",
        "--raw-stream", path, "--exit-code", str(code),
    ], text=True)
    value = json.loads(out)
    assert value["state"] == state, value
    if failure is None:
        assert "failure_reason" not in value, value
    else:
        assert value["failure_reason"] == failure, value
    if blocker is not None:
        assert value["finalization_code"] == blocker, value
if json.loads(subprocess.check_output([
    "wg", "classify-failure", "--terminal", "--json", "--executor", "pi",
    "--raw-stream", sys.argv[5], "--exit-code", "1",
], text=True))["reason_code"] != "conflicting-exact-terminal-receipts":
    raise AssertionError("ambiguous receipt reason was not preserved")
PY

echo "terminal receipt precedence smoke passed"
