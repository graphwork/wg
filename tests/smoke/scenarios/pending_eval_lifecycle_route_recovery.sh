#!/usr/bin/env bash
# Route-stable FailedPendingEval lifecycle regression. Credential-free: it
# exercises real CLI scaffold/completion/dispatcher diagnostics while every
# service tick uses --max-agents 0, so no external model is invoked.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
export WG_SMOKE_AGENT_OVERRIDE=1

scratch=$(make_scratch)
cd "$scratch"
wg init --route codex-cli >/dev/null
wg config --auto-assign false --auto-evaluate true --flip-enabled true --no-reload >/dev/null

wg add "route-stable source" --id route-source --no-place \
  -d $'## Validation\n- [ ] durable verdict required' >/dev/null
wg pause route-source >/dev/null
wg service tick --max-agents 0 >/dev/null
wg resume route-source >/dev/null

python3 - <<'PY'
import json
rows=[json.loads(x) for x in open('.wg/graph.jsonl') if x.strip()]
tasks={r['id']:r for r in rows if r.get('kind') == 'task'}
for tid in ('.flip-route-source','.evaluate-route-source'):
    t=tasks[tid]
    assert t['model'].startswith('codex:'), (tid,t.get('model'))
    p=t['agency_dispatch']
    assert p['plan_hash'].startswith('b3:')
    assert p['pipeline_id'].startswith('evalp-')
    assert all(c['route'].startswith('codex:') for c in p['calls'])
    assert all(c['system']['handler'] == 'codex' for c in p['calls'])
assert len(tasks['.flip-route-source']['agency_dispatch']['calls']) == 2
PY

# Scaffold two more explicit handlers. The persisted plans must retain their
# complete route even after config changes again; provider metadata may never
# choose a handler implicitly.
for route_case in \
  'pi|pi:openai-codex:gpt-5.6-sol|pi' \
  'claude|claude:haiku|claude'
do
  IFS='|' read -r label route handler <<<"$route_case"
  wg config --local --set-model evaluator "$route" \
    --set-model flip_inference "$route" \
    --set-model flip_comparison "$route" --no-reload >/dev/null
  wg add "route matrix $label" --id "route-$label" --no-place \
    -d $'## Validation\n- [ ] persisted route identity' >/dev/null
  wg pause "route-$label" >/dev/null
  wg service tick --max-agents 0 >/dev/null
  wg resume "route-$label" >/dev/null
  LABEL="$label" ROUTE="$route" HANDLER="$handler" python3 - <<'PY'
import json,os
rows=[json.loads(x) for x in open('.wg/graph.jsonl') if x.strip()]
tasks={r['id']:r for r in rows if r.get('kind') == 'task'}
label,route,handler=(os.environ[k] for k in ('LABEL','ROUTE','HANDLER'))
for prefix in ('.flip-','.evaluate-'):
    t=tasks[prefix+'route-'+label]
    p=t['agency_dispatch']
    assert t['model'] == route,(t['id'],t['model'],route)
    assert all(c['route'] == route for c in p['calls']),p
    assert all(c['system']['handler'] == handler for c in p['calls']),p
PY
  wg pause "route-$label" >/dev/null
done

# Drift ambient config once more. The Codex source's already-persisted plan is
# still Codex; invocation/restart reads it rather than this new Claude selection.
python3 - <<'PY'
import json
rows=[json.loads(x) for x in open('.wg/graph.jsonl') if x.strip()]
tasks={r['id']:r for r in rows if r.get('kind') == 'task'}
assert all(c['route'].startswith('codex:') for c in tasks['.evaluate-route-source']['agency_dispatch']['calls'])
PY

wg claim route-source >/dev/null
wg fail route-source --class agent-exit-nonzero --reason 'synthetic wrapper failure' >/dev/null
ready=$(wg ready)
grep -q '.flip-route-source' <<<"$ready" || loud_fail "FLIP not dispatcher-ready: $ready"
why=$(wg why-blocked .flip-route-source)
grep -q 'dispatcher-ready via evaluation-system bypass' <<<"$why" || loud_fail "why-blocked disagrees with dispatcher: $why"
if grep -q 'ROOT CAUSE' <<<"$why"; then loud_fail "soft gate was mislabeled as SCC/root cycle: $why"; fi

# Luca's retained predicate: the owning rescue stage can complete over a
# FailedPendingEval source, but an ordinary dependent cannot.
wg done .flip-route-source --ignore-unmerged-worktree --skip-smoke >/dev/null
wg add 'ordinary dependent' --id ordinary --after route-source --no-place >/dev/null
if wg done ordinary --ignore-unmerged-worktree --skip-smoke >/tmp/ordinary.out 2>&1; then
  loud_fail 'ordinary dependent bypassed FailedPendingEval gate'
fi

# A terminal satellite with no durable verdict must never scorelessly promote
# the source. This is a real dispatcher maintenance tick, with spawning disabled.
wg done .evaluate-route-source --ignore-unmerged-worktree --skip-smoke >/dev/null
wg service tick --max-agents 1 >/dev/null
status=$(wg show route-source --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["status"])')
[[ "$status" == 'failed-pending-eval' ]] || loud_fail "scoreless promotion occurred: $status"

# Reproduce the historical bare-Codex pre-claim breaker row. The reconciler may
# normalize this losslessly once and rearm it, but must remain idempotent.
python3 - <<'PY'
import json,os,tempfile
p='.wg/graph.jsonl'; rows=[]
for line in open(p):
    r=json.loads(line)
    if r.get('kind') == 'task' and r.get('id') == '.evaluate-route-source':
        r['status']='incomplete'; r['model']='gpt-5.4-mini'; r['provider']='codex'
        r.pop('agency_dispatch',None); r.pop('evaluation_lifecycle',None)
        r['spawn_failures']=5; r['assigned']=None; r['failure_reason']='legacy bare inline route'; r['not_before']='2099-01-01T00:00:00Z'
    rows.append(r)
fd,tmp=tempfile.mkstemp(dir='.wg',prefix='graph-smoke-',text=True)
with os.fdopen(fd,'w') as f:
    for r in rows: f.write(json.dumps(r,separators=(',',':'))+'\n')
os.replace(tmp,p)
PY
wg service tick --max-agents 1 >/dev/null
python3 - <<'PY'
import json
rows=[json.loads(x) for x in open('.wg/graph.jsonl') if x.strip()]
t=next(r for r in rows if r.get('kind')=='task' and r.get('id')=='.evaluate-route-source')
assert t['status']=='open',t
assert t['model']=='codex:gpt-5.4-mini',t
assert t.get('spawn_failures',0)==0,t
assert t['evaluation_lifecycle']['repair_version']==1,t
print(sum('Installed lossless historical plan' in e.get('message','') for e in t.get('log',[])))
PY
before=$(python3 - <<'PY'
import json
r=next(x for x in map(json.loads,open('.wg/graph.jsonl')) if x.get('kind')=='task' and x.get('id')=='.evaluate-route-source')
print(sum('Installed lossless historical plan' in e.get('message','') for e in r.get('log',[])))
PY
)
wg service tick --max-agents 1 >/dev/null
after=$(python3 - <<'PY'
import json
r=next(x for x in map(json.loads,open('.wg/graph.jsonl')) if x.get('kind')=='task' and x.get('id')=='.evaluate-route-source')
print(sum('Installed lossless historical plan' in e.get('message','') for e in r.get('log',[])))
PY
)
[[ "$before" == "$after" ]] || loud_fail "historical repair was not idempotent: $before -> $after"

# Slow storage plus overlapping/restarted maintenance ticks must remain a
# bounded no-op after the one CAS repair. Holding graph.lock widens the stale
# snapshot window exactly like the incident harness; no agent directory or
# additional repair may appear.
agent_dirs_before=$( (find .wg/agents -mindepth 1 -maxdepth 1 -type d 2>/dev/null || true) | wc -l )
if command -v flock >/dev/null 2>&1; then
  start_ms=$(date +%s%3N)
  flock -x .wg/graph.lock -c 'sleep 1' &
  lock_pid=$!
  sleep 0.1
  wg service tick --max-agents 1 >/tmp/pending-tick-a.log 2>&1 & a=$!
  wg service tick --max-agents 1 >/tmp/pending-tick-b.log 2>&1 & b=$!
  wait "$lock_pid" "$a" "$b"
  elapsed=$(( $(date +%s%3N) - start_ms ))
  (( elapsed >= 800 )) || loud_fail "slow-storage harness did not hold ticks: ${elapsed}ms"
fi
for _ in 1 2 3 4 5; do
  wg service tick --max-agents 1 >/dev/null
done
agent_dirs_after=$( (find .wg/agents -mindepth 1 -maxdepth 1 -type d 2>/dev/null || true) | wc -l )
[[ "$agent_dirs_before" == "$agent_dirs_after" ]] || loud_fail \
  "restart/slow-tick respawn storm: agent dirs $agent_dirs_before -> $agent_dirs_after"
final_repairs=$(python3 - <<'PY'
import json
r=next(x for x in map(json.loads,open('.wg/graph.jsonl')) if x.get('kind')=='task' and x.get('id')=='.evaluate-route-source')
print(sum('Installed lossless historical plan' in e.get('message','') for e in r.get('log',[])))
PY
)
[[ "$before" == "$final_repairs" ]] || loud_fail \
  "restart ticks repeated historical CAS: $before -> $final_repairs"

# Pre-schema parents could already be PendingEval with a claimed evaluator that
# finished and persisted exactly one post-start Evaluation, but neither row had
# plan/lifecycle metadata. A restart must losslessly link that evidence once;
# it must not rearm the claimed row, spawn a replacement, or require approval.
wg add 'legacy completed source' --id legacy-completed --no-place >/dev/null
wg add 'legacy completed evaluator' --id .evaluate-legacy-completed --no-place >/dev/null
python3 - <<'PY'
import datetime,json,os,tempfile
now=datetime.datetime.now(datetime.timezone.utc)
p='.wg/graph.jsonl'; rows=[]
for line in open(p):
    r=json.loads(line)
    if r.get('kind') == 'task' and r.get('id') == 'legacy-completed':
        r['status']='pending-eval'; r['assigned']='agent-legacy-worker'
        r['started_at']=(now-datetime.timedelta(seconds=30)).isoformat()
        r['completed_at']=(now-datetime.timedelta(seconds=20)).isoformat()
        r.pop('agency_dispatch',None); r.pop('evaluation_lifecycle',None)
    if r.get('kind') == 'task' and r.get('id') == '.evaluate-legacy-completed':
        r['status']='done'; r['model']='pi:openai-codex:gpt-5.6-terra'
        r['assigned']='agent-legacy-evaluator'
        r['started_at']=(now-datetime.timedelta(seconds=10)).isoformat()
        r['completed_at']=(now-datetime.timedelta(seconds=5)).isoformat()
        r.pop('agency_dispatch',None); r.pop('evaluation_lifecycle',None)
    rows.append(r)
fd,tmp=tempfile.mkstemp(dir='.wg',prefix='graph-legacy-completed-',text=True)
with os.fdopen(fd,'w') as f:
    for r in rows: f.write(json.dumps(r,separators=(',',':'))+'\n')
os.replace(tmp,p)
PY
wg evaluate record --task legacy-completed --score 0.91 --source llm \
  --notes 'one completed claimed legacy evaluator result' >/dev/null
legacy_evals_before=$(python3 - <<'PY'
import glob,json
print(sum(json.load(open(p)).get('task_id') == 'legacy-completed'
          for p in glob.glob('.wg/agency/evaluations/*.json')))
PY
)
[[ "$legacy_evals_before" == 1 ]] || loud_fail "legacy fixture has $legacy_evals_before evaluations, expected one"
wg service tick --max-agents 1 >/dev/null
python3 - <<'PY'
import glob,json
rows=[json.loads(x) for x in open('.wg/graph.jsonl') if x.strip()]
tasks={r['id']:r for r in rows if r.get('kind') == 'task'}
source=tasks['legacy-completed']; evaluator=tasks['.evaluate-legacy-completed']
assert source['status']=='done',source
assert source['evaluation_lifecycle']['consumed_verdict'],source
assert evaluator['status']=='done',evaluator
assert evaluator['agency_dispatch']['calls'][0]['route']=='pi:openai-codex:gpt-5.6-terra',evaluator
assert evaluator['evaluation_lifecycle']['linked_eval_verdict'],evaluator
verdicts=[]
for p in glob.glob('.wg/agency/eval-lifecycle/verdicts/*.json'):
    v=json.load(open(p))
    if v.get('source_task') == 'legacy-completed': verdicts.append(v)
assert len(verdicts)==1,verdicts
PY
for _ in 1 2 3; do wg service tick --max-agents 1 >/dev/null; done
legacy_evals_after=$(python3 - <<'PY'
import glob,json
print(sum(json.load(open(p)).get('task_id') == 'legacy-completed'
          for p in glob.glob('.wg/agency/evaluations/*.json')))
PY
)
[[ "$legacy_evals_after" == 1 ]] || loud_fail \
  "legacy completed evaluator reran after restart: $legacy_evals_before -> $legacy_evals_after"

echo 'PASS: Codex/Pi/Claude plans, relation-aware rescue, verdict gate, legacy completed-evaluator recovery, diagnostics, bounded slow/restart repair'
