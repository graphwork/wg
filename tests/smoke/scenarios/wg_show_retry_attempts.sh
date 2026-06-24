#!/usr/bin/env bash
# Scenario: wg_show_retry_attempts
#
# Regression (rshow): `wg show <retried-task>`'s "Attempt History" must
# attribute each archived attempt to (a) the agent that ran it and (b) the
# evaluation that actually scored it. Two bugs shipped in the original
# print_retry_history:
#
#   1. Per-attempt agent id NEVER rendered. The agent id was extracted with
#      `output.txt`.lines().take(5).split_whitespace().find(starts_with "agent-")
#      — but the archived init record is COMPACT JSON (no inter-token
#      whitespace) with the id buried inside `"cwd":".../agent-<n>"`, so the
#      whitespace scan always returned None and no `[agent-<n>]` label appeared.
#   2. Per-attempt eval was wrong. find_eval_for_attempt ignored the archive
#      timestamp and returned the single NEWEST eval for EVERY attempt, so a
#      retried attempt 1 displayed attempt 2's score.
#
# This drives the real human flow — the `wg show` CLI on a real `.wg` layout
# with two archived attempts and two evals — and asserts the rendered text.
# It also covers BOTH agent-id resolution paths: attempt 1 relies on the
# output.txt fallback scanner; attempt 2 has an authoritative `agent-id` file
# whose value must WIN over a deliberately-divergent cwd in its output.txt.
#
# Requires: python3 (to mint the synthetic graph/archives/evals) and wg.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 needed to build the synthetic .wg layout"
fi

scratch=$(make_scratch)
cd "$scratch"

if ! wg init -x claude >init.log 2>&1; then
    loud_fail "wg init failed during smoke setup: $(tail -5 init.log)"
fi

graph_dir="$scratch/.wg"
if [[ ! -f "$graph_dir/graph.jsonl" ]]; then
    loud_fail "could not locate graph.jsonl under .wg/ after init"
fi

if ! wg add "Retried show task" --id smoke-rshow >add.log 2>&1; then
    loud_fail "wg add failed during smoke setup: $(tail -5 add.log)"
fi

# Mark the task failed + retried so print_retry_history renders.
python3 - "$graph_dir/graph.jsonl" <<'PY'
import json, sys
path = sys.argv[1]
out = []
for line in open(path):
    if not line.strip():
        continue
    obj = json.loads(line)
    if obj.get("kind") == "task" and obj.get("id") == "smoke-rshow":
        obj["status"] = "failed"
        obj["assigned"] = None
        obj["retry_count"] = 2
        obj["failure_reason"] = "smoke fixture"
    out.append(json.dumps(obj))
open(path, "w").write("\n".join(out) + "\n")
PY

# Two archived attempts. Attempt 1 (older) carries its agent id ONLY inside the
# compact-JSON cwd of output.txt (exercises the fallback scanner, no agent-id
# file). Attempt 2 (newer) has an authoritative agent-id file = agent-200 whose
# value must beat the divergent agent-999 in its own output.txt cwd.
a1="$graph_dir/log/agents/smoke-rshow/2026-01-01T10:00:00Z"
a2="$graph_dir/log/agents/smoke-rshow/2026-01-01T11:00:00Z"
mkdir -p "$a1" "$a2"

cat >"$a1/output.txt" <<'EOF'
{"type":"system","subtype":"init","cwd":"/home/u/.wg-worktrees/agent-100","session_id":"s1","tools":["Bash"]}
{"type":"assistant","message":{"content":[{"type":"text","text":"attempt one"}]}}
EOF
: >"$a1/prompt.txt"

cat >"$a2/output.txt" <<'EOF'
{"type":"system","subtype":"init","cwd":"/home/u/.wg-worktrees/agent-999","session_id":"s2","tools":["Bash"]}
{"type":"assistant","message":{"content":[{"type":"text","text":"attempt two"}]}}
EOF
: >"$a2/prompt.txt"
printf 'agent-200' >"$a2/agent-id"

# Two evals. Internal timestamps place eval A inside attempt 1's window
# (10:00–11:00) and eval B after attempt 2's archive (>= 11:00).
evals="$graph_dir/agency/evaluations"
mkdir -p "$evals"

write_eval() {
    local file="$1" score="$2" ts="$3" notes="$4"
    python3 - "$file" "$score" "$ts" "$notes" <<'PY'
import json, sys
file, score, ts, notes = sys.argv[1:5]
json.dump({
    "task_id": "smoke-rshow",
    "score": float(score),
    "notes": notes,
    "evaluator": "smoke",
    "source": "llm",
    "timestamp": ts,
}, open(file, "w"))
PY
}

write_eval "$evals/eval-smoke-rshow-2026-01-01T10-30-00.json" 0.10 \
    "2026-01-01T10:30:00+00:00" "first attempt was wrong"
write_eval "$evals/eval-smoke-rshow-2026-01-01T11-30-00.json" 0.88 \
    "2026-01-01T11:30:00+00:00" "second attempt fixed it"

# --- Drive the real CLI flow ---
out="$scratch/show.txt"
if ! wg show smoke-rshow >"$out" 2>&1; then
    loud_fail "wg show failed:\n$(cat "$out")"
fi

hist=$(sed -n '/Attempt History/,/^$/p' "$out")
if [[ -z "$hist" ]]; then
    loud_fail "no Attempt History section in wg show output:\n$(cat "$out")"
fi

line1=$(printf '%s\n' "$hist" | grep 'Attempt 1:' || true)
line2=$(printf '%s\n' "$hist" | grep 'Attempt 2:' || true)
[[ -n "$line1" ]] || loud_fail "missing 'Attempt 1:' line:\n$hist"
[[ -n "$line2" ]] || loud_fail "missing 'Attempt 2:' line:\n$hist"

# Bug 1: agent attribution. Attempt 1 via output.txt fallback scanner.
case "$line1" in
    *"[agent-100]"*) ;;
    *) loud_fail "Attempt 1 should show [agent-100] (fallback scan of compact-JSON cwd).\n$line1" ;;
esac
# Attempt 2: authoritative agent-id file (agent-200) WINS over output cwd (agent-999).
case "$line2" in
    *"[agent-200]"*) ;;
    *) loud_fail "Attempt 2 should show [agent-200] from the authoritative agent-id file.\n$line2" ;;
esac
case "$line2" in
    *agent-999*) loud_fail "agent-id file must win over output.txt cwd (saw agent-999).\n$line2" ;;
esac

# Bug 2: per-attempt eval attribution — each attempt shows ONLY its own eval.
case "$line1" in
    *"0.10"*) ;;
    *) loud_fail "Attempt 1 should show its own eval 0.10.\n$line1" ;;
esac
case "$line1" in
    *"0.88"*) loud_fail "Attempt 1 must NOT show attempt 2's eval 0.88 (the bug).\n$line1" ;;
esac
case "$line2" in
    *"0.88"*) ;;
    *) loud_fail "Attempt 2 should show its own eval 0.88.\n$line2" ;;
esac
case "$line2" in
    *"0.10"*) loud_fail "Attempt 2 must NOT show attempt 1's eval 0.10.\n$line2" ;;
esac

echo "PASS: wg show attributes each attempt to its agent and its own eval"
exit 0
