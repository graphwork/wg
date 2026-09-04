#!/usr/bin/env bash
# A receipt-backed Done/Landed source is authoritative before migration.
set -euo pipefail
source "$(dirname "$0")/_helpers.sh"
: "${WG_BIN:?smoke harness must provide candidate WG_BIN}"
[[ -x $WG_BIN ]] || loud_fail "candidate WG_BIN is not executable: $WG_BIN"

export WG_SMOKE_ROOT="${WG_EVAL_CUTOVER_SELF_HEAL_ROOT:-/tmp/wgsmoke-evaluation-self-heal-$$}"
scratch=$(make_scratch "evaluation-cutover-self-heal")
project="$scratch/project"
home="$scratch/home"
mkdir -p "$project" "$home"
export HOME="$home" XDG_CONFIG_HOME="$home/.config"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH WG_DIR

git -C "$project" init -q -b main
git -C "$project" config user.email cutover-self-heal@test.invalid
git -C "$project" config user.name CutoverSelfHeal
printf 'base\n' >"$project/README.md"
git -C "$project" add README.md
git -C "$project" commit -qm base
cd "$project"
wgrun() { env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC WG_DIR="$project/.wg" "$WG_BIN" "$@"; }

wgrun init --no-agency --route pi --model pi:test:unused >/dev/null
wgrun config set dispatcher.settling_delay_ms 0 >/dev/null
wgrun config set dispatcher.worktree_isolation false >/dev/null
wgrun add 'Receipt-backed source' --id source >/dev/null
wgrun add 'Retired stale evaluator' --id .evaluate-source --after source --priority 100 >/dev/null
marker="$project/downstream-dispatched"
wgrun add 'Downstream' --id downstream --after source \
  --exec "printf dispatched > '$marker'" --exec-mode shell >/dev/null
wgrun publish source --only >/dev/null
wgrun publish .evaluate-source --only >/dev/null
wgrun publish downstream --only >/dev/null
wgrun done source --operator-accept --reason 'typed receipt for legacy-upgrade regression' >/dev/null

# Modern completion eagerly abandons synthetic rows. Restore the historical
# Open projection (and remove only that row's ledger records) to emulate an
# upgraded pre-cutover graph without manufacturing source acceptance.
python3 - "$project/.wg/graph.jsonl" "$project/.wg/lifecycle/events.jsonl" <<'PY'
import json,os,sys

graph_path,ledger_path=sys.argv[1:]
rows=[json.loads(line) for line in open(graph_path)]
legacy=next(row for row in rows if row.get('id')=='.evaluate-source')
legacy['status']='open'
legacy.pop('completed_at',None)
legacy.pop('lifecycle',None)
with open(graph_path+'.tmp','w') as out:
    for row in rows:
        out.write(json.dumps(row,separators=(',',':'))+'\n')
os.replace(graph_path+'.tmp',graph_path)
if os.path.exists(ledger_path):
    events=[json.loads(line) for line in open(ledger_path)]
    with open(ledger_path+'.tmp','w') as out:
        for event in events:
            if event.get('event',{}).get('task_id')!='.evaluate-source':
                out.write(json.dumps(event,separators=(',',':'))+'\n')
    os.replace(ledger_path+'.tmp',ledger_path)
PY

[[ ! -e $project/.wg/migrations/evaluation-cutover-v1 ]] \
  || loud_fail 'fixture unexpectedly ran evaluation-cutover migration'
wgrun ready --json | grep -q 'downstream' \
  || loud_fail 'receipt-backed Done/Landed source remained blocked by stale evaluator'

start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --no-supervise --interval 1
for _ in $(seq 1 240); do
  [[ -f $marker ]] && break
  sleep 0.05
done
[[ -f $marker ]] \
  || loud_fail "downstream did not dispatch without migration: $(tail -100 "$project/.wg/service/daemon.log" 2>/dev/null || true)"
[[ ! -e $project/.wg/migrations/evaluation-cutover-v1/backups ]] \
  || loud_fail 'dispatcher secretly mutated graph through migration'

python3 - "$project/.wg/graph.jsonl" <<'PY'
import json,sys
rows=[json.loads(line) for line in open(sys.argv[1])]
source=next(row for row in rows if row.get('id')=='source')
evaluator=next(row for row in rows if row.get('id')=='.evaluate-source')
downstream=next(row for row in rows if row.get('id')=='downstream')
assert source['status']=='done',source
assert source.get('completion_disposition')=='landed',source
assert source.get('completion_receipt'),source
# The restored coordinator may eagerly archive the row after proving it is
# synthetic; either projection remains visible and non-authoritative.
assert evaluator['status'] in ('open','abandoned'),evaluator
assert downstream['status'] in ('in-progress','done'),downstream
PY

notices=$(grep -c 'retired synthetic agency row .evaluate-source was excluded before priority ordering' "$project/.wg/service/daemon.log" 2>/dev/null || true)
[[ $notices -le 1 ]] || loud_fail "retired row emitted per-tick spam ($notices notices)"
if grep -qE 'Priority (bump|inheritance|dispatch order).*\.evaluate-source' "$project/.wg/service/daemon.log"; then
  loud_fail 'retired row entered promotion/order/dispatch accounting'
fi

echo 'PASS: receipt-backed Done/Landed source dispatched downstream with stale evaluator present and no evaluation-cutover migration'
