#!/usr/bin/env bash
# Installed-binary assign -> execute/review -> score -> reward/evolver -> changed assignment.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

scratch=$(make_scratch)
repo_root="$(cd "$HERE/../../.." && pwd)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then WG_BIN="$WG_SMOKE_CANDIDATE_BIN"; else
  export CARGO_TARGET_DIR="$scratch/target"
  (cd "$repo_root" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
  WG_BIN="$CARGO_TARGET_DIR/debug/wg"
fi
project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$project" "$home" "$fakebin"
cat >"$fakebin/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
prompt=$(cat || true)
if grep -q 'overall_score' <<<"$prompt"; then
  text='{"overall_score":0.93,"dimensions":{"correctness":0.93,"completeness":0.93,"efficiency":0.93,"style_adherence":0.93,"downstream_usability":0.93,"coordination_overhead":0.93,"blocking_impact":0.93},"notes":"independent terminal evidence is strong"}'
else
  text='{"verdict":"pass","findings":[]}'
fi
python3 - "$text" <<'PY'
import json,sys
text=sys.argv[1]
print(json.dumps({'type':'turn_end','message':{'role':'assistant','content':[{'type':'text','text':text}],
 'provider':'test','model':'adaptive-loop','stopReason':'stop','usage':{'input':5,'output':2,
 'cacheRead':0,'cacheWrite':0,'totalTokens':7,'cost':{'total':0.001}}}},separators=(',',':')))
PY
FAKE_PI
chmod +x "$fakebin/pi"
export HOME="$home" WG_GLOBAL_DIR="$home/.wg" PATH="$fakebin:$PATH"
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_EXECUTOR_TYPE WG_MODEL WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH
cd "$project"
git init -q -b main; git config user.email adaptive@test.invalid; git config user.name Adaptive
printf 'base\n' >base.txt; git add base.txt; git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
G="$project/.wg"; wgrun(){ "$WG_BIN" --dir "$G" "$@"; }
wgrun agency init >/dev/null
wgrun config --local --model pi:test:source-worker --reasoning low --auto-assign false --auto-evaluate false \
  --set-model reviewer pi:test:completion-review --set-reasoning reviewer low \
  --set-model evaluator pi:test:completion-eval --set-reasoning evaluator low --no-reload >/dev/null

# Ensure there are two implementation-capable compositions.
role=$(wgrun --json role list | python3 -c 'import json,sys; print(next(x["id"] for x in json.load(sys.stdin) if x["name"]=="Programmer"))')
read -r trade1 trade2 < <(wgrun --json tradeoff list | python3 -c 'import json,sys; a=json.load(sys.stdin); print(a[0]["id"],a[1]["id"])')
wgrun agent create adaptive-a --role "$role" --tradeoff "$trade1" >/dev/null 2>&1 || true
wgrun agent create adaptive-b --role "$role" --tradeoff "$trade2" >/dev/null 2>&1 || true
mapfile -t programmers < <(python3 - "$G/agency/cache/roles" "$G/agency/cache/agents" "$role" <<'PY'
import json,subprocess,sys,os
# Agent JSON is obtained through the installed CLI by the caller instead; this fallback reads
# stable YAML id/role lines without a YAML dependency.
roles,agents,role=sys.argv[1:]
for name in sorted(os.listdir(agents)):
    if not name.endswith('.yaml'): continue
    text=open(os.path.join(agents,name)).read()
    rid=''
    for line in text.splitlines():
        if line.startswith('role_id:'):
            rid=line.split(':',1)[1].strip().strip('"\'')
    if rid==role: print(name[:-5])
PY
)
[[ ${#programmers[@]} -ge 2 ]] || loud_fail "agency init did not yield two Programmer compositions"

# Capture deterministic baseline choice before any compatible delayed reward.
wgrun add baseline --id baseline -d $'Baseline ranking probe.\n\n## Validation\n- [ ] probe' >/dev/null
wgrun assign baseline --auto >"$scratch/baseline.log" 2>&1
baseline_agent=$(python3 - "$G/graph.jsonl" <<'PY'
import json,sys
print(next(x['agent'] for x in map(json.loads,open(sys.argv[1])) if x['id']=='baseline'))
PY
)
target=''
for id in "${programmers[@]}"; do [[ "$id" != "$baseline_agent" ]] && target="$id" && break; done
[[ -n "$target" ]] || loud_fail "could not choose a non-baseline composition"

# Explicitly bind and execute a real reviewed source attempt for the other composition.
wgrun add learned --id learned -d $'Produce a report.\n\n## Validation\n- [ ] reviewed terminal evidence' >/dev/null
wgrun contract learned report >/dev/null
wgrun assign learned "$target" >/dev/null
wgrun publish learned --only >/dev/null
wgrun claim learned --actor fixture-worker >/dev/null
printf 'complete result\n' >report.txt; printf 'summary\n' >summary.txt; printf 'validation\n' >validation.log
wgrun completion-object report.txt --media-type text/plain >output-ref.json
wgrun completion-object validation.log --media-type text/plain --evidence-kind validation >evidence-ref.json
wgrun completion-manifest learned --summary summary.txt --output-ref output-ref.json --evidence-ref evidence-ref.json >manifest.json
wgrun submit learned --manifest manifest.json --summary summary.txt >/dev/null
wgrun done learned >/dev/null
# Completion FLIP/Eval used their snapshotted routes above. Move the evaluator
# role to a disjoint terminal-outcome route before scoring; same-route reviewer
# self-opinion must not become assignment reward.
wgrun config --local --set-model evaluator pi:test:outcome-scorer --set-reasoning evaluator low --no-reload >/dev/null
wgrun evaluate run learned >/dev/null

learning=$(wgrun learning show learned)
if ! grep -q 'Delayed assignment reward: 0.930' <<<"$learning"; then
  printf '%s\n' "$learning" >&2
  find "$G/agency/adaptive/v1/outcome-assessments" -name '*.json' -type f -exec sh -c 'for f do cat "$f" >&2; done' sh {} +
  loud_fail "delayed reward was not projected"
fi
grep -q 'Evolver input: projected=true' <<<"$learning" || loud_fail "reward was not projected into the evolver input manifest"

# The next deterministic automatic assignment must change to the newly learned composition.
wgrun add improved --id improved -d $'Next compatible work item.\n\n## Validation\n- [ ] use learned ranking' >/dev/null
wgrun assign improved --auto >"$scratch/improved.log" 2>&1
next_agent=$(python3 - "$G/graph.jsonl" <<'PY'
import json,sys
print(next(x['agent'] for x in map(json.loads,open(sys.argv[1])) if x['id']=='improved'))
PY
)
[[ "$next_agent" == "$target" ]] || loud_fail "prior learning did not change future assignment: baseline=$baseline_agent target=$target next=$next_agent"
grep -q 'Deterministic receipt-backed ranking' "$scratch/improved.log" || loud_fail "terminal help/output still misstates selector behavior"

# Optional automatic admission uses the same bounded lane and never a graph blocker.
wgrun config --local --auto-assign true --no-reload >/dev/null
wgrun add auto-admit --id auto-admit -d $'Automatic admission probe.\n\n## Validation\n- [ ] receipt before claim' >/dev/null
wgrun publish auto-admit --only >/dev/null
wgrun claim auto-admit --actor fixture-auto >/dev/null
wgrun config --local --auto-assign false --no-reload >/dev/null
wgrun add direct --id direct -d $'Direct dispatch probe.\n\n## Validation\n- [ ] explicit uncomposed marker' >/dev/null
wgrun publish direct --only >/dev/null
wgrun claim direct --actor fixture-direct >/dev/null

# Receipt binds the real attempt, and the graph contains no synthetic agency rows.
python3 - "$G" "$target" <<'PY'
import glob,json,os,sys
g,target=sys.argv[1:]
receipts=[json.load(open(p)) for p in glob.glob(g+'/agency/adaptive/v1/assignment-receipts/*.json')]
r=next(x for x in receipts if x['task_id']=='learned')
assert r['attempt_id'].startswith('attempt-') and r['attempt_fence']>0,r
assert r['decision']['kind']=='explicit' and r['selected_composition']['agent_id']==target,r
auto=next(x for x in receipts if x['task_id']=='auto-admit')
assert auto['decision']['kind']=='automatic' and auto['selected_composition']['agent_id']==target,auto
assert auto['selection_id'] and os.path.exists(g+'/agency/adaptive/v1/assignment-selection/'+auto['selection_id'].removeprefix('b3:')+'.json'),auto
direct=next(x for x in receipts if x['task_id']=='direct')
assert direct['decision']['kind']=='uncomposed' and direct.get('selected_composition') is None,direct
rows=[json.loads(x) for x in open(g+'/graph.jsonl')]
auto_row=next(x for x in rows if x['id']=='auto-admit')
reservation=next(x for x in auto_row['lifecycle']['audit'] if x['event_kind']=='attempt-reserved')
assert auto['receipt_id'] in reservation['evidence_refs'],reservation
rewards=[json.load(open(p)) for p in glob.glob(g+'/agency/adaptive/v1/assignment-rewards/*.json')]
assert len(rewards)==1 and abs(rewards[0]['reward']-.93)<1e-9,rewards
assert not any(x['id'].startswith(('.assign-','.flip-','.evaluate-','.evolve-')) for x in rows),rows
PY

echo 'PASS: attempt receipt -> terminal FLIP/Eval -> delayed reward/evolver -> improved deterministic assignment, with zero synthetic agency rows'
