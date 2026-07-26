#!/usr/bin/env bash
# Installed-binary terminal flow for fix-no-cli. Exercises the exact operator
# surface that previously required graph.jsonl surgery: status visibility,
# single-task retry, batch recover, and stable unsatisfiable reconciliation.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

scratch=$(make_scratch)
cd "$scratch"
wg init >/dev/null

graph=""
for candidate in .wg .workgraph; do
  if [[ -f "$candidate/graph.jsonl" ]]; then
    graph="$candidate/graph.jsonl"
    break
  fi
done
[[ -n "$graph" ]] || loud_fail "wg init did not create graph.jsonl"

# These are persisted-state fixtures only; every mutation under test below is
# performed through wg itself. `loop-pin` models the recurring historical gate
# log loop, while the other rows exercise sanctioned operator recovery.
python3 - "$graph" <<'PY'
import json,sys
path=sys.argv[1]
diag=("error[WG-EVAL-PIPELINE-AMBIGUOUS]: operator action required: "
      "current-attempt gate is unsatisfiable")
rows=[]
for task_id,status in [
    ("retry-pending", "pending-eval"),
    ("retry-failed-pending", "failed-pending-eval"),
    ("recover-failed-pending", "failed-pending-eval"),
    ("loop-pin", "pending-eval"),
]:
    rows.append({
        "kind":"task", "id":task_id, "title":task_id, "status":status,
        "created_at":"2026-07-26T00:00:00+00:00",
        "evaluation_lifecycle":{
            "schema":1, "pipeline_id":f"evalp-{task_id}-attempt-1",
            "source_attempt":1, "route_generation":0,
            "schedule_attempts":0, "transport_attempts":0,
            "semantic_attempts":0, "execution_state":"blocked",
            "repair_version":1, "repair_attempts":1,
            "diagnostic":diag,
        },
    })
with open(path,"a") as out:
    for row in rows:
        out.write(json.dumps(row)+"\n")
PY

# Visibility: both real serialized status values must be accepted by `wg list`.
pending_list=$(wg list --status pending-eval)
grep -q 'retry-pending' <<<"$pending_list" \
  || loud_fail "wg list --status pending-eval did not show held task: $pending_list"
failed_list=$(wg list --status failed-pending-eval)
grep -q 'retry-failed-pending' <<<"$failed_list" \
  || loud_fail "wg list --status failed-pending-eval did not show held task: $failed_list"

# Reconciliation must leave an already-unsatisfiable historical gate stable.
# Before the fix every tick appended another "Pinned historical PendingEval"
# row even though operator action was already required.
for _ in 1 2 3; do
  wg service tick --max-agents 0 >/dev/null
 done
python3 - "$graph" <<'PY'
import json,sys
row=next(r for r in map(json.loads,open(sys.argv[1]))
         if r.get("kind")=="task" and r.get("id")=="loop-pin")
life=row["evaluation_lifecycle"]
assert life.get("gate_policy") is None, life
assert "operator action required" in life.get("diagnostic",""), life
pins=[e for e in row.get("log",[]) if "Pinned historical PendingEval" in e.get("message","")]
assert pins == [], pins
PY

# Single-task sanctioned recovery for both evaluation-held statuses.
wg retry retry-pending --reason 'operator resolved ambiguous eval gate' >retry-pending.log
wg retry retry-failed-pending --reason 'operator resolved ambiguous rescue eval gate' >retry-failed.log
grep -q 'Cleared stuck evaluation gate' retry-pending.log \
  || loud_fail "retry did not report the sanctioned gate clear"
grep -q 'Cleared stuck evaluation gate' retry-failed.log \
  || loud_fail "failed-pending retry did not report the sanctioned gate clear"

# Batch recovery must accept failed-pending-eval as a filter and apply it.
wg recover --filter status=failed-pending-eval --yes \
  --reason 'operator batch-cleared ambiguous eval gate' >recover.log
grep -q 'recover-failed-pending' recover.log \
  || loud_fail "recover filter did not select failed-pending-eval"

python3 - "$graph" <<'PY'
import json,sys
rows={r["id"]:r for r in map(json.loads,open(sys.argv[1])) if r.get("kind")=="task"}
for task_id in ("retry-pending","retry-failed-pending","recover-failed-pending"):
    row=rows[task_id]
    assert row["status"]=="open", (task_id,row)
    life=row["evaluation_lifecycle"]
    assert life["source_attempt"]==2, (task_id,life)
    assert life.get("diagnostic") is None, (task_id,life)
    assert life.get("repair_attempts",0)==0, (task_id,life)
PY

for task_id in retry-pending retry-failed-pending recover-failed-pending; do
  show=$(wg show "$task_id" --json)
  python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["status"]=="open",d; assert d.get("evaluation_health") is None,d' <<<"$show"
done

echo 'PASS: pending-eval and failed-pending-eval are visible and recoverable through wg CLI; unsatisfiable reconciliation is stable'
