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
# This scenario materialises graph tasks in PendingEval and FailedPendingEval
# states, runs representative `wg evaluate run` / `--flip` commands, and asserts
# the status precondition error string is NOT present in stderr. The commands
# may still exit non-zero or time out for other reasons (no agent / no role /
# FLIP disabled / no LLM endpoint), but they MUST get past the status check.
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

# Synthetic eval-pending tasks. Task `c` intentionally uses Rust debug spelling
# to pin status normalization during disk/state recovery.
cat >>"$graph_dir/graph.jsonl" <<'EOF'
{"kind":"task","id":"a","title":"PendingEval parent","status":"pending-eval","created_at":"2026-04-27T17:00:00+00:00"}
{"kind":"task","id":"b","title":"FailedPendingEval parent","status":"failed-pending-eval","created_at":"2026-04-27T17:00:00+00:00"}
{"kind":"task","id":"c","title":"FailedPendingEval debug spelling","status":"FailedPendingEval","created_at":"2026-04-27T17:00:00+00:00"}
EOF

run_case() {
    local label="$1"
    shift
    local err="$label.err"
    local out="$label.out"

    # Timeout is acceptable here: the status check happens before any slow LLM
    # path. A timeout means the command got past the precondition under test.
    timeout 8 wg evaluate run "$@" >"$out" 2>"$err"
    rc=$?
    if grep -Eq "has status (PendingEval|FailedPendingEval|pending-eval|failed-pending-eval)" "$err"; then
        loud_fail "wg evaluate run rejects eval-pending status for $label (precondition still fires):\n$(cat "$err")"
    fi
    if grep -q "unknown variant.*FailedPendingEval" "$err"; then
        loud_fail "wg evaluate run failed to normalize FailedPendingEval status spelling for $label:\n$(cat "$err")"
    fi
    if grep -q "must be done, failed, or pending-eval to evaluate" "$err"; then
        loud_fail "wg evaluate run still emits stale allowed-status list for $label:\n$(cat "$err")"
    fi
    if grep -q "must be done or failed to evaluate" "$err"; then
        loud_fail "wg evaluate run still emits old pre-PendingEval message for $label:\n$(cat "$err")"
    fi
}

run_case pending-eval a
run_case failed-pending-eval b
run_case failed-pending-eval-debug-flip c --flip

echo "PASS: wg evaluate run accepts PendingEval and FailedPendingEval input states (last rc=$rc)"
exit 0
