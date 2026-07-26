#!/usr/bin/env bash
# Credential-free installed-binary terminal flow for `wg edit --clear-route-pin`.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

unset WG_AGENT_ID WG_TASK_ID WG_EXECUTOR_TYPE WG_MODEL WG_REASONING WG_TIER
scratch=$(make_scratch)
project="$scratch/project"
home="$scratch/home"
mkdir -p "$project" "$home"
export HOME="$home"
export WG_GLOBAL_DIR="$home/.wg"
cd "$project"
git init -q -b main
git config user.email smoke@example.invalid
git config user.name 'WG Smoke'
touch seed.txt
git add seed.txt
git commit -q -m seed

run_wg() {
  env -u WG_DIR -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH -u WG_WORKTREE_ACTIVE \
    -u WG_BRANCH -u WG_AGENT_ID -u WG_TASK_ID -u WG_EXECUTOR_TYPE -u WG_MODEL \
    HOME="$HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" wg "$@"
}

run_wg edit --help | grep -q -- '--clear-route-pin' \
  || loud_fail 'edit help does not expose --clear-route-pin'
run_wg edit --help | grep -q -- 'unlike.*retry --current-profile\|does not retry or snapshot' \
  || loud_fail 'edit help does not contrast route clearing with retry --current-profile'
run_wg init >/dev/null 2>&1
run_wg profile create profile-a -m 'pi:openai-codex:worker-a' >/dev/null
run_wg profile create profile-b -m 'codex:worker-b' >/dev/null
run_wg profile select profile-a --no-reload >/dev/null

# Seed every compatibility selector plus immutable historical evidence. This is
# fixture setup only; the actual user flow below is the supported CLI command.
run_wg add 'Pinned route' --id pinned \
  --model 'pi:openrouter:stale-worker' --reasoning low >/dev/null
mkdir -p .wg/service .wg-worktrees
git worktree add -q .wg-worktrees/agent-history -b wg/agent-history/pinned HEAD
printf 'historic WIP\n' >.wg-worktrees/agent-history/uncommitted-wip.txt
python3 - <<'PY'
import json
path='.wg/graph.jsonl'
rows=[json.loads(line) for line in open(path) if line.strip()]
for row in rows:
    if row.get('kind')=='task' and row['id']=='pinned':
        row.update({
            'status':'failed', 'provider':'legacy-provider', 'endpoint':'legacy-endpoint',
            'profile':'legacy-wcc-profile', 'tier':'premium',
            'session_id':'stale-route-session', 'assigned':'agent-history',
            'retry_count':3, 'failure_reason':'historic failure',
            'token_usage':{'cost_usd':1.25,'input_tokens':100,'output_tokens':20},
            'artifacts':['historic-result.txt'],
        })
        row.setdefault('log',[]).append({
            'timestamp':'2026-01-01T00:00:00Z','actor':'worker',
            'message':'historic attempt provenance'
        })
with open(path,'w') as f:
    for row in rows: f.write(json.dumps(row,separators=(',',':'))+'\n')
registry={
  'agents':{
    'agent-history':{
      'id':'agent-history','pid':999999,'task_id':'pinned','executor':'pi',
      'started_at':'2026-01-01T00:00:00Z','last_heartbeat':'2026-01-01T00:00:00Z',
      'status':'dead','output_file':'/tmp/history/output.log',
      'model':'openrouter:historic-actual','completed_at':'2026-01-01T01:00:00Z',
      'worktree_path':None
    }
  },
  'next_agent_id':1
}
open('.wg/service/registry.json','w').write(json.dumps(registry))
PY

before=$(run_wg show pinned --json)
python3 -c 'import json,sys; d=json.load(sys.stdin); assert d["route_pin"]["state"]=="exact-task-pin",d' <<<"$before"

out=$(run_wg edit pinned --clear-route-pin)
grep -q 'Dynamic inheritance (not pinned): profile=profile-a' <<<"$out" \
  || loud_fail "clear output omitted dynamic profile inheritance: $out"
grep -q 'model=pi:openai-codex:worker-a' <<<"$out" \
  || loud_fail "clear output omitted current non-pinning route preview: $out"
test -f .wg-worktrees/agent-history/uncommitted-wip.txt \
  || loud_fail 'clear-route-pin touched the historical worktree/WIP'

# One command clears all selectors while historical actual route, status,
# usage, provenance, evidence, assignment, and worktree remain unchanged.
after=$(run_wg show pinned --json)
AFTER_JSON="$after" python3 - <<'PY'
import json,os
d=json.loads(os.environ['AFTER_JSON'])
assert d['status']=='failed',d
assert d['retry_count']==3,d
assert d['failure_reason']=='historic failure',d
assert d['token_usage']['cost_usd']==1.25,d
assert d['artifacts']==['historic-result.txt'],d
assert d['assigned']=='agent-history',d
assert d['actual_executor']=='pi',d
assert d['actual_model']=='openrouter:historic-actual',d
assert 'model' not in d,d
assert 'reasoning' not in d,d
assert 'session_id' not in d,d
pin=d['route_pin']
assert pin['state']=='inherited-unpinned',pin
assert pin['dynamic_at_dispatch'] is True,pin
assert pin['pinned_fields']==[],pin
assert pin['current_inheritance']['profile']=='profile-a',pin
assert pin['current_inheritance']['route']=='pi:openai-codex:worker-a',pin
entry=next(e for e in d['log'] if e.get('actor')=='clear-route-pin')
for field in ('model','reasoning','provider','endpoint','profile','tier','session_id'):
    assert field in entry['message'],(field,entry)
assert 'no route snapshot was written' in entry['message'],entry
assert any(e['message']=='historic attempt provenance' for e in d['log']),d['log']
PY
human=$(run_wg show pinned)
grep -q 'Route pin: inherited/unpinned' <<<"$human" \
  || loud_fail "human show does not distinguish unpinned inheritance: $human"
grep -q 'Current inheritance preview: profile=profile-a' <<<"$human" \
  || loud_fail "human show omitted current profile preview: $human"

# A profile flip BEFORE dispatch changes the preview because the task contains
# no snapshot. This is the intentional opposite of retry --current-profile.
run_wg profile select profile-b --no-reload >/dev/null
flipped=$(run_wg show pinned --json)
FLIPPED_JSON="$flipped" python3 - <<'PY'
import json,os
d=json.loads(os.environ['FLIPPED_JSON'])
pin=d['route_pin']
assert pin['state']=='inherited-unpinned',pin
assert pin['current_inheritance']['profile']=='profile-b',pin
assert pin['current_inheritance']['route']=='codex:worker-b',pin
assert 'model' not in d,d
assert 'reasoning' not in d,d
PY

# Safe in-progress flow: the running attempt stays in-progress and keeps its
# actual route/session in registry/audit, while future selectors are removed.
run_wg add 'Live pinned route' --id live \
  --model 'pi:openrouter:live-pin' --reasoning medium >/dev/null
python3 - <<'PY'
import json
path='.wg/graph.jsonl'
rows=[json.loads(line) for line in open(path) if line.strip()]
for row in rows:
    if row.get('kind')=='task' and row['id']=='live':
        row.update({'status':'in-progress','assigned':'agent-live','started_at':'2026-01-02T00:00:00Z','session_id':'active-session','provider':'legacy-live'})
with open(path,'w') as f:
    for row in rows: f.write(json.dumps(row,separators=(',',':'))+'\n')
r=json.load(open('.wg/service/registry.json'))
r['agents']['agent-live']={
  'id':'agent-live','pid':999998,'task_id':'live','executor':'pi',
  'started_at':'2026-01-02T00:00:00Z','last_heartbeat':'2026-01-02T00:00:00Z',
  'status':'working','output_file':'/tmp/live/output.log','model':'openrouter:live-actual',
  'worktree_path':None
}
open('.wg/service/registry.json','w').write(json.dumps(r))
PY
run_wg edit live --clear-route-pin >/dev/null
live=$(run_wg show live --json)
LIVE_JSON="$live" python3 - <<'PY'
import json,os
d=json.loads(os.environ['LIVE_JSON'])
assert d['status']=='in-progress',d
assert d['assigned']=='agent-live',d
assert d['actual_executor']=='pi',d
assert d['actual_model']=='openrouter:live-actual',d
assert 'model' not in d and 'reasoning' not in d and 'session_id' not in d,d
assert d['route_pin']['state']=='inherited-unpinned',d['route_pin']
assert d['route_pin']['applies_to']=='future-attempt',d['route_pin']
a=next(e for e in d['log'] if e.get('actor')=='clear-route-pin')
assert 'active_attempt_agent=agent-live' in a['message'],a
assert 'actual_executor=pi' in a['message'],a
assert 'actual_model=openrouter:live-actual' in a['message'],a
assert 'actual_session=active-session' in a['message'],a
PY

# Fail closed when an in-progress record cannot be represented safely.
run_wg add 'Unsafe live route' --id unsafe-live \
  --model 'pi:openrouter:must-remain' --reasoning low >/dev/null
python3 - <<'PY'
import json
p='.wg/graph.jsonl'; rows=[json.loads(x) for x in open(p) if x.strip()]
for r in rows:
    if r.get('kind')=='task' and r['id']=='unsafe-live':
        r.update({'status':'in-progress','assigned':'missing-agent','session_id':'must-remain'})
with open(p,'w') as f:
    for r in rows:f.write(json.dumps(r,separators=(',',':'))+'\n')
PY
if run_wg edit unsafe-live --clear-route-pin >"$scratch/unsafe.out" 2>&1; then
  loud_fail 'unrecorded in-progress route clear unexpectedly succeeded'
fi
grep -q 'refused' "$scratch/unsafe.out" \
  || loud_fail "missing fail-closed diagnostic: $(cat "$scratch/unsafe.out")"
unsafe=$(run_wg show unsafe-live --json)
UNSAFE_JSON="$unsafe" python3 - <<'PY'
import json,os
d=json.loads(os.environ['UNSAFE_JSON'])
assert d['model']=='pi:openrouter:must-remain',d
assert d['reasoning']=='low',d
assert d['session_id']=='must-remain',d
assert d['status']=='in-progress',d
PY

echo 'PASS: edit --clear-route-pin atomically unpins future execution, preserves history/live actual route, follows later profile flips, renders clearly, and fails closed for unsafe in-progress state'
