#!/usr/bin/env bash
# Upgrade one legacy graph, then run the receipt-backed adaptive loop in place.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"
: "${WG_BIN:?smoke harness must provide candidate WG_BIN}"
[[ -x $WG_BIN ]] || loud_fail "candidate WG_BIN is not executable: $WG_BIN"

# Keep daemon socket paths below sockaddr_un.sun_path's platform limit.
export WG_SMOKE_ROOT="${WG_ADAPTIVE_AGENCY_MIGRATION_ROOT:-/tmp/wgsmoke-adaptive-agency-$$}"
scratch=$(make_scratch)
project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$project" "$home" "$fakebin"
cat >"$fakebin/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
prompt=$(cat || true)
if grep -q 'overall_score' <<<"$prompt"; then
  text='{"overall_score":0.94,"dimensions":{"correctness":0.94,"completeness":0.94,"efficiency":0.94,"style_adherence":0.94,"downstream_usability":0.94,"coordination_overhead":0.94,"blocking_impact":0.94},"notes":"independent upgraded-graph outcome"}'
else
  text='{"verdict":"pass","findings":[]}'
fi
python3 - "$text" <<'PY'
import json,sys
print(json.dumps({'type':'turn_end','message':{'role':'assistant','content':[{'type':'text','text':sys.argv[1]}],
 'provider':'test','model':'adaptive-migration-cycle','stopReason':'stop','usage':{'input':5,'output':2,
 'cacheRead':0,'cacheWrite':0,'totalTokens':7,'cost':{'total':0.001}}}},separators=(',',':')))
PY
FAKE_PI
chmod +x "$fakebin/pi"
export HOME="$home" WG_GLOBAL_DIR="$home/.wg" PATH="$fakebin:$PATH"
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_EXECUTOR_TYPE WG_MODEL WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH WG_DIR
cd "$project"
git init -q -b main; git config user.email adaptive-migration@test.invalid; git config user.name AdaptiveMigration
printf 'base\n' >base.txt; git add base.txt; git commit -qm base
"$WG_BIN" init --no-agency --route pi --model pi:test:unused >/dev/null
G="$project/.wg"; wgrun(){ "$WG_BIN" --dir "$G" "$@"; }
wgrun config set dispatcher.settling_delay_ms 0 >/dev/null
wgrun config set dispatcher.worktree_isolation false >/dev/null
wgrun config --local --model pi:test:source-worker --reasoning low --auto-assign false --auto-evaluate false \
  --set-model reviewer pi:test:completion-review --set-reasoning reviewer low \
  --set-model evaluator pi:test:completion-eval --set-reasoning evaluator low --no-reload >/dev/null

# Supported old fixture boundary. After this write, every transition is through
# the candidate CLI; the test performs no dependency deletion or graph surgery.
wgrun add legacy-source --id legacy-source >/dev/null
wgrun add legacy-evaluator --id .evaluate-legacy-source --after legacy-source >/dev/null
wgrun add legacy-downstream --id legacy-downstream --after legacy-source \
  --exec "printf released > '$scratch/downstream-ran'" --exec-mode shell >/dev/null
wgrun publish legacy-source --only >/dev/null
wgrun publish .evaluate-legacy-source --only >/dev/null
wgrun publish legacy-downstream --only >/dev/null
python3 - "$G/graph.jsonl" <<'PY'
import json,sys
p=sys.argv[1]; rows=[]
for line in open(p):
    row=json.loads(line)
    if row['id']=='legacy-source':
        row['status']='done'; row['completion_disposition']='landed'; row['completed_at']='2026-01-01T00:00:00Z'
        row.setdefault('log',[]).append({'timestamp':'2026-01-01T00:00:00Z','actor':'legacy-review','message':'preserve this historical review'})
    elif row['id']=='.evaluate-legacy-source':
        row['status']='open'; row['paused']=False; row['completed_at']=None
    rows.append(row)
with open(p,'w') as f:
    for row in rows: f.write(json.dumps(row,separators=(',',':'))+'\n')
PY
cp "$G/graph.jsonl" "$scratch/legacy.graph.jsonl"

# An unrelated outcome score is explicitly non-authoritative and cannot release
# the stale dependency. The versioned migration does, while preserving bytes.
wgrun evaluate record --task legacy-source --score 1.0 --source manual >/dev/null
wgrun ready --json | grep -q 'legacy-downstream' && loud_fail 'external outcome score forged candidate acceptance'
dry=$(wgrun migrate evaluation-cutover --dry-run --json)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["operation_kind"]=="legacy_evaluation_cutover" and x["dry_run"] is True,x' <<<"$dry"
cmp -s "$G/graph.jsonl" "$scratch/legacy.graph.jsonl" || loud_fail 'cutover dry-run changed graph bytes'
apply=$(wgrun migrate evaluation-cutover --json)
backup=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["backup_path"])' <<<"$apply")
[[ -f $backup ]] || loud_fail "cutover backup missing: $apply"
cmp -s "$backup" "$scratch/legacy.graph.jsonl" || loud_fail 'cutover backup did not preserve exact legacy graph'
grep -q 'preserve this historical review' "$backup" || loud_fail 'legacy log missing from exact backup'
wgrun ready --json | grep -q 'legacy-downstream' || loud_fail 'migration did not restore downstream liveness'
cp "$G/graph.jsonl" "$scratch/migrated.graph.jsonl"
wgrun migrate evaluation-cutover --json >/dev/null
cmp -s "$G/graph.jsonl" "$scratch/migrated.graph.jsonl" || loud_fail 'cutover replay changed graph bytes'
start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --no-supervise --interval 1
for _ in $(seq 1 200); do
  [[ -f $scratch/downstream-ran ]] && break
  sleep 0.05
done
[[ -f $scratch/downstream-ran ]] || loud_fail "released downstream did not dispatch through the real CLI: $(tail -100 "$G/service/daemon.log" 2>/dev/null || true)"
wgrun service stop >/dev/null

# Seed two compatible compositions and capture the pre-learning deterministic rank.
wgrun agency init >/dev/null
role=$(wgrun --json role list | python3 -c 'import json,sys; print(next(x["id"] for x in json.load(sys.stdin) if x["name"]=="Programmer"))')
read -r trade1 trade2 < <(wgrun --json tradeoff list | python3 -c 'import json,sys; a=json.load(sys.stdin); print(a[0]["id"],a[1]["id"])')
wgrun agent create cycle-a --role "$role" --tradeoff "$trade1" >/dev/null 2>&1 || true
wgrun agent create cycle-b --role "$role" --tradeoff "$trade2" >/dev/null 2>&1 || true
mapfile -t programmers < <(python3 - "$G/agency/cache/agents" "$role" <<'PY'
import os,sys
agents,role=sys.argv[1:]
for name in sorted(os.listdir(agents)):
    if not name.endswith('.yaml'): continue
    text=open(os.path.join(agents,name)).read(); rid=''
    for line in text.splitlines():
        if line.startswith('role_id:'): rid=line.split(':',1)[1].strip().strip('"\'')
    if rid==role: print(name[:-5])
PY
)
[[ ${#programmers[@]} -ge 2 ]] || loud_fail 'fixture needs two Programmer compositions'
wgrun add baseline --id baseline -d $'Baseline probe.\n\n## Validation\n- [ ] deterministic rank' >/dev/null
wgrun assign baseline --auto >/dev/null
baseline=$(python3 - "$G/graph.jsonl" <<'PY'
import json,sys
print(next(x['agent'] for x in map(json.loads,open(sys.argv[1])) if x['id']=='baseline'))
PY
)
target=''; for id in "${programmers[@]}"; do [[ $id != "$baseline" ]] && target="$id" && break; done
[[ -n $target ]] || loud_fail 'could not choose non-baseline composition'

# Complete one real receipt-backed attempt on the non-baseline composition.
wgrun add learned --id learned -d $'Produce an upgraded-graph report.\n\n## Validation\n- [ ] receipt-backed review' >/dev/null
wgrun contract learned report >/dev/null
wgrun assign learned "$target" >/dev/null
wgrun publish learned --only >/dev/null
wgrun claim learned --actor fixture-worker >/dev/null
printf 'upgraded graph result\n' >report.txt; printf 'summary\n' >summary.txt; printf 'validation\n' >validation.log
wgrun completion-object report.txt --media-type text/plain >output-ref.json
wgrun completion-object validation.log --media-type text/plain --evidence-kind validation >evidence-ref.json
wgrun completion-manifest learned --summary summary.txt --output-ref output-ref.json --evidence-ref evidence-ref.json >manifest.json
wgrun submit learned --manifest manifest.json --summary summary.txt >/dev/null
wgrun done learned >/dev/null
wgrun config --local --set-model evaluator pi:test:independent-outcome --set-reasoning evaluator low --no-reload >/dev/null
score=$(wgrun --json evaluate run learned)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["operation_kind"]=="scored_outcome_evaluation",x' <<<"$score"
learning=$(wgrun --json learning show learned)
python3 -c 'import json,sys; x=json.load(sys.stdin); assert x["operation_kind"]=="terminal_learning_episode" and x["task_id"]=="learned",x' <<<"$learning"
reviews=$(wgrun --json reviews list learned)
python3 -c 'import json,sys; a=json.load(sys.stdin); assert len(a)==2 and all(x["operation_kind"]=="candidate_review" for x in a),a' <<<"$reviews"

# The delayed reward changes the next deterministic choice. Automatic admission
# uses the same selector and binds its decision to the real attempt receipt.
wgrun add improved --id improved -d $'Use prior compatible learning.\n\n## Validation\n- [ ] changed rank' >/dev/null
wgrun assign improved --auto >/dev/null
next=$(python3 - "$G/graph.jsonl" <<'PY'
import json,sys
print(next(x['agent'] for x in map(json.loads,open(sys.argv[1])) if x['id']=='improved'))
PY
)
[[ $next == "$target" ]] || loud_fail "learning did not change rank: baseline=$baseline target=$target next=$next"
wgrun config --local --auto-assign true --no-reload >/dev/null
wgrun add auto-admit --id auto-admit -d $'Auto admission.\n\n## Validation\n- [ ] attempt receipt' >/dev/null
wgrun publish auto-admit --only >/dev/null
wgrun claim auto-admit --actor fixture-auto >/dev/null

# Human-facing help, status, and config all expose the same authority map.
quick=$(wgrun quickstart)
grep -q 'AGENCY FEEDBACK & MIGRATION' <<<"$quick" || loud_fail 'quickstart omitted authority workflow'
grep -q 'Only the completion controller applies candidate receipts' <<<"$quick" || loud_fail 'quickstart omitted lifecycle authority'
help=$(wgrun --help-all); grep -q 'Score already-terminal outcomes for learning' <<<"$help" || loud_fail 'full top-level help conflated outcome score with completion'
eval_help=$(wgrun evaluate --help); grep -q 'cannot accept a candidate' <<<"$eval_help" || loud_fail 'evaluate help omitted outcome scope'
config_help=$(wgrun config --help); grep -q 'bounded pre-claim identity selection' <<<"$config_help" || loud_fail 'config help omitted receipt-backed auto assignment'
config=$(wgrun config --show); grep -q '\[agency authority\]' <<<"$config" || loud_fail 'config show omitted authority map'
grep -q 'bounded pre-claim selection with an attempt receipt' <<<"$config" || loud_fail 'config show misstated auto assignment'
status=$(wgrun --json status)
python3 -c 'import json,sys; x=json.load(sys.stdin); a=x["agency_authority"]; assert "completion controller alone" in a["completion_review"] and "no task/lifecycle authority" in a["scored_outcome"]; s=x["adaptive"]; assert s["assignment_receipts"]>=2 and s["terminal_episodes"]>=1 and s["outcome_assessments"]==1 and s["active_assignment_rewards"]==1,(a,s)' <<<"$status"

# Exactly one synthetic row remains: the preserved, inert historical evidence.
python3 - "$G" "$target" <<'PY'
import glob,json,os,sys
g,target=sys.argv[1:]
rows=[json.loads(x) for x in open(g+'/graph.jsonl')]
synthetic=[x for x in rows if x['id'].startswith(('.assign-','.flip-','.evaluate-','.evolve-'))]
assert len(synthetic)==1 and synthetic[0]['id']=='.evaluate-legacy-source',synthetic
assert 'evaluation-cutover:v1:historical-inert' in synthetic[0].get('tags',[]),synthetic[0]
receipts=[json.load(open(p)) for p in glob.glob(g+'/agency/adaptive/v1/assignment-receipts/*.json')]
auto=next(x for x in receipts if x['task_id']=='auto-admit')
assert auto['decision']['kind']=='automatic' and auto['selected_composition']['agent_id']==target,auto
reservation=next(e for e in next(x for x in rows if x['id']=='auto-admit')['lifecycle']['audit'] if e['event_kind']=='attempt-reserved')
assert auto['receipt_id'] in reservation['evidence_refs'],reservation
rewards=[json.load(open(p)) for p in glob.glob(g+'/agency/adaptive/v1/assignment-rewards/*.json')]
assert len(rewards)==1 and abs(rewards[0]['reward']-.94)<1e-9,rewards
PY

echo 'PASS: one upgraded graph migrated in place, preserved legacy evidence, restored liveness, and completed receipt -> review -> episode -> score -> reward -> changed assignment with one authority map'
