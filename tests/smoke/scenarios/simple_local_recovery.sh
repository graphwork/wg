#!/usr/bin/env bash
# Golden-path proof for the simplified trusted-local recovery lifecycle.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"

scratch=$(make_scratch)
repo="$scratch/project"; home="$scratch/home"
mkdir -p "$repo" "$home"
ROOT="$(cd "$HERE/../../.." && pwd)"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$ROOT/target/debug/wg}"
[[ -x "$WG_BIN" ]] || (cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"

unset WG_GRAPH_ID WG_WORKER_ATTEMPT_ID WG_WORKER_ATTEMPT_FENCE WG_WORKER_GENERATION \
  WG_SPAWN_EPOCH WG_SPAWN_RUN_ID WG_BRANCH WG_WORKER_CONTROL_MODE WG_WORKTREE_ACTIVE || true
clean_env=(env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH HOME="$home")
run(){ (cd "$repo" && "${clean_env[@]}" "$WG_BIN" --dir "$repo/.wg" "$@"); }

cd "$repo"
git init -q -b main
git config user.email recovery@test.invalid
git config user.name Recovery
echo base > base.txt
git add base.txt && git commit -qm base
"${clean_env[@]}" "$WG_BIN" init --no-agency >/dev/null
if "${clean_env[@]}" "$WG_BIN" --help | grep -Eq '^  (completion-object|completion-manifest|submit|land|finalize|candidate)'; then
  loud_fail "internal completion ceremony leaked into normal top-level help"
fi
git add .gitignore AGENTS.md CLAUDE.md && git commit -qm init-wg

run add "Simple completion" --id simple-finish >/dev/null
run publish simple-finish --only >/dev/null
run claim simple-finish --actor local-worker >/dev/null
git switch -qc worker/simple-finish
echo complete > result.txt
git add result.txt && git commit -qm result
if ! (cd "$repo" && env -u WG_DIR HOME="$home" WG_TASK_ID=simple-finish WG_AGENT_ID=local-worker \
  "$WG_BIN" --dir "$repo/.wg" done simple-finish >"$scratch/done.out" 2>"$scratch/done.err"); then
  loud_fail "ordinary Done failed: $(cat "$scratch/done.err")"
fi
run show simple-finish --json >"$scratch/simple.json"
run list --all >"$scratch/list.out"
grep -E 'simple-finish.*\(assign ✓ · flip \?' "$scratch/list.out" >/dev/null \
  || loud_fail "parent task row omitted compact assignment/review activity"
python3 - "$scratch/simple.json" "$repo/.wg/graph.jsonl" <<'PY'
import json, sys
x=json.load(open(sys.argv[1]))
assert x["status"] == "done", x
assert x["completion_disposition"] == "landed", x
assert x["completion_receipt"], x
activity=x.get("completion_review_activity", [])
assert len(activity) == 1, activity
assert activity[0]["reviewer_kind"] == "flip", activity
assert activity[0]["verdict"] == "unavailable", activity
rows=[json.loads(line) for line in open(sys.argv[2]) if line.strip()]
tasks=[row for row in rows if row.get("kind") == "task"]
assert not any(row.get("status") in {"pending-eval", "failed-pending-eval"} for row in tasks), tasks
assert not any(row.get("id", "").startswith((".assign-", ".flip-", ".evaluate-")) for row in tasks), tasks
PY
[[ "$(git rev-parse main)" == "$(git rev-parse worker/simple-finish)" ]] || loud_fail "one-operation completion did not publish exact worker commit"
grep -q "Advisory model review did not pass" "$scratch/done.err" || loud_fail "advisory finding was not visible"

# Explicit operator recovery is forbidden inside worker authority, then succeeds
# outside it with an immutable receipt and reason.
run add "Dead owner recovery" --id operator-recovery >/dev/null
run publish operator-recovery --only >/dev/null
run claim operator-recovery --actor dead-worker >/dev/null
if (cd "$repo" && env -u WG_DIR HOME="$home" WG_AGENT_ID=dead-worker \
  "$WG_BIN" --dir "$repo/.wg" done operator-recovery --operator-accept --reason bad \
  >"$scratch/operator-worker.out" 2>"$scratch/operator-worker.err"); then
  loud_fail "worker environment acquired operator acceptance authority"
fi
grep -q "operator acceptance is refused inside a worker process" "$scratch/operator-worker.err" || loud_fail "worker refusal was not explicit"
run done operator-recovery --operator-accept --reason "operator verified preserved result" >"$scratch/operator.out"
run show operator-recovery --json >"$scratch/operator.json"
python3 - "$scratch/operator.json" "$repo/.wg/completion/v3/objects" <<'PY'
import json, pathlib, sys
x=json.load(open(sys.argv[1]))
assert x["status"] == "done", x
receipt=x["completion_receipt"]
assert receipt and receipt.startswith("b3:"), x
p=pathlib.Path(sys.argv[2]) / receipt.removeprefix("b3:")
assert p.is_file(), p
body=json.loads(p.read_text())
assert body["reason"] == "operator verified preserved result", body
PY

# Accepted terminal outcomes enter Agency through a separate create-once,
# unscored observation ledger. Repeated Done, an explicit migration/backfill,
# and a daemon-style tick must not duplicate either episode.
run agency stats >"$scratch/agency-stats.out"
run --json agency stats >"$scratch/agency-stats.json"
grep -q 'Terminal observations: 2' "$scratch/agency-stats.out" \
  || loud_fail "agency stats omitted terminal observation count"
grep -q 'Operator-accepted:    1' "$scratch/agency-stats.out" \
  || loud_fail "agency stats did not distinguish operator acceptance"
python3 - "$scratch/agency-stats.json" <<'PY'
import json, sys
x=json.load(open(sys.argv[1]))
o=x["overview"]
assert o["total_evaluations"] == 0, o
assert o["avg_score"] is None, o
assert o["total_terminal_observations"] == 2, o
assert o["unscored_terminal_observations"] == 2, o
assert o["operator_accepted_terminal_observations"] == 1, o
rows=x["terminal_outcomes"]
assert len(rows) == 2, rows
assert all(row["score_state"] == "unscored" and "score" not in row for row in rows), rows
by_kind={row["acceptance_kind"]: row for row in rows}
assert set(by_kind) == {"reviewed_completion", "operator_accepted"}, by_kind
normal=by_kind["reviewed_completion"]
assert normal["reviewed_completion"]["publication_receipt"].startswith("git:"), normal
assert normal["execution"]["lifecycle_actor"] == "completion-v3", normal
assert normal["current_candidate_review_disagreement"] is True, normal
assert normal["review_trajectory_disagreement"] is True, normal
assert any(review["verdict"] == "unavailable" for review in normal["reviews"]), normal
operator=by_kind["operator_accepted"]
assert operator["operator_acceptance"]["reason"] == "operator verified preserved result", operator
assert operator["operator_acceptance"]["ordinary_publication_verified"] is False, operator
PY
run done simple-finish >/dev/null
run done operator-recovery --operator-accept --reason "operator verified preserved result" >/dev/null
run agency migrate >/dev/null
run service tick --max-agents 0 >/dev/null
run --json agency stats >"$scratch/agency-replayed.json"
python3 - "$scratch/agency-replayed.json" <<'PY'
import json, sys
x=json.load(open(sys.argv[1]))
assert x["overview"]["total_terminal_observations"] == 2, x["overview"]
assert len(x["terminal_outcomes"]) == 2, x["terminal_outcomes"]
PY

# Explicit strict review remains available. Replaying an immutable candidate
# reuses its receipt, while two materially distinct rejected commits consume
# the candidate-revision budget; the third revision parks successfully without
# another model call or source failure.
mkdir -p "$scratch/fakebin"
cat >"$scratch/fakebin/pi" <<'SH'
#!/usr/bin/env bash
cat >/dev/null || true
printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"{\"verdict\":\"reject\",\"findings\":[{\"code\":\"strict.test\",\"message\":\"repair required\"}]}"}],"provider":"test","model":"fake-review","stopReason":"stop","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}'
SH
chmod +x "$scratch/fakebin/pi"
cat >"$repo/.wg/config.toml" <<'TOML'
[agency]
auto_assign = false
auto_evaluate = false
completion_review_strict = true
gate_max_attempts = 2

[models.reviewer]
model = "pi:openrouter:fake-review"
reasoning = "low"

[models.evaluator]
model = "pi:openrouter:fake-review"
reasoning = "low"
TOML
run add "Bounded strict review" --id strict-review >/dev/null
run publish strict-review --only >/dev/null
run claim strict-review --actor strict-worker >/dev/null
git switch -qc worker/strict-review main
echo strict > strict.txt
git add strict.txt && git commit -qm strict
for attempt in 1 2; do
  if (cd "$repo" && env -u WG_DIR HOME="$home" PATH="$scratch/fakebin:$PATH" \
    WG_TASK_ID=strict-review WG_AGENT_ID=strict-worker "$WG_BIN" --dir "$repo/.wg" \
    done strict-review >"$scratch/strict-$attempt.out" 2>"$scratch/strict-$attempt.err"); then
    loud_fail "strict semantic rejection unexpectedly published on attempt $attempt"
  fi
  grep -q 'strict.test.*repair required' "$scratch/strict-$attempt.err" \
    || loud_fail "strict rejection did not return exact actionable finding: $(cat "$scratch/strict-$attempt.err")"
  printf 'revision-%s\n' "$attempt" >> strict.txt
  git add strict.txt && git commit -qm "strict revision $attempt"
done
if ! (cd "$repo" && env -u WG_DIR HOME="$home" PATH="$scratch/fakebin:$PATH" \
  WG_TASK_ID=strict-review WG_AGENT_ID=strict-worker "$WG_BIN" --dir "$repo/.wg" \
  done strict-review >"$scratch/strict-3.out" 2>"$scratch/strict-3.err"); then
  loud_fail "strict review ceiling failed source work instead of parking: $(cat "$scratch/strict-3.err")"
fi
grep -q 'Needs review: strict model-review attempt limit (2) reached' "$scratch/strict-3.err" \
  || loud_fail "bounded strict exhaustion was not visible"
run show strict-review --json >"$scratch/strict.json"
python3 - "$scratch/strict.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1]))
assert x["status"] == "waiting", x
assert x.get("assigned") is None, x
assert len(x.get("completion_review_activity", [])) == 2, x
assert x["completion_blocker"]["kind"] == "needs-review", x
assert "semantic review ceiling 2/2" in x["completion_blocker"]["reason"], x
assert not x.get("failure_reason"), x
assert any("Completion waiting/NeedsReview" in row["message"] for row in x["log"]), x
PY

echo "PASS: simple local completion, advisory visibility, audited recovery, and bounded strict review"
