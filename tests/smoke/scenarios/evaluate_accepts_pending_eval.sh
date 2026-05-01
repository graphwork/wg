#!/usr/bin/env bash
# Scenario: evaluate_accepts_pending_eval
#
# Regression: between 2026-04-27 ~15:29 and ~17:00, every `.flip-X` and
# `.evaluate-X` task auto-failed with:
#
#   Error: Task '<parent>' has status PendingEval — must be done or failed to evaluate
#
# Cause: `wg evaluate run` rejected `Status::PendingEval` as a precondition,
# but the dispatcher correctly fires `.evaluate-X`/`.flip-X` while parent is
# still PendingEval (per the eval-gated unblock contract). The eval command
# must accept PendingEval as a valid input state — it means "done but eval
# pending", which is the entire point of the eval-gate.
#
# This scenario materialises a graph with a single PendingEval task, runs
# `wg evaluate run a` and `wg evaluate run a --flip`, and asserts the
# precondition error string is NOT present in stderr. The commands may still
# exit non-zero for other reasons (no agent / no role / FLIP disabled), but
# they MUST get past the status check.
#
# Fast (no daemon, no LLM, no network).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
trap 'rm -rf "$scratch"' EXIT
cd "$scratch"

if ! wg init -x shell >init.log 2>&1; then
    loud_fail "wg init failed during smoke setup: $(tail -5 init.log)"
fi

graph_dir=""
for cand in .wg .wg; do
    if [[ -f "$scratch/$cand/graph.jsonl" ]]; then
        graph_dir="$scratch/$cand"
        break
    fi
done
if [[ -z "$graph_dir" ]]; then
    loud_fail "could not locate graph.jsonl under .wg/ or .wg/ after init"
fi

# Synthetic PendingEval task.
cat >>"$graph_dir/graph.jsonl" <<'EOF'
{"kind":"task","id":"a","title":"PendingEval parent","status":"pending-eval","created_at":"2026-04-27T17:00:00+00:00"}
EOF

# Bare `wg evaluate run a` — must not bail with the precondition error.
wg evaluate run a >eval.out 2>eval.err
ec=$?
if grep -q "has status PendingEval" eval.err; then
    loud_fail "wg evaluate run rejects PendingEval (precondition still fires):\n$(cat eval.err)"
fi
if grep -q "must be done or failed to evaluate" eval.err; then
    loud_fail "wg evaluate run still emits stale error message:\n$(cat eval.err)"
fi

# `wg evaluate run a --flip` — same contract.
wg evaluate run a --flip >flip.out 2>flip.err
fc=$?
if grep -q "has status PendingEval" flip.err; then
    loud_fail "wg evaluate run --flip rejects PendingEval:\n$(cat flip.err)"
fi
if grep -q "must be done or failed to evaluate" flip.err; then
    loud_fail "wg evaluate run --flip still emits stale error message:\n$(cat flip.err)"
fi

echo "PASS: wg evaluate run accepts PendingEval as a valid input state (run ec=$ec, flip ec=$fc)"
exit 0
