#!/usr/bin/env bash
# Candidate-binary regression for a registered, proven-dead spawn owner whose
# dirty worktree, private owner token, and observer state must be retained while
# the next bounded attempt reuses the checkout without breaker/falloff charge.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-${CARGO_TARGET_DIR:-$(cd "$HERE/../../.." && pwd)/target}/debug/wg}"
[[ -x "$WG_BIN" ]] || loud_fail "candidate wg missing: $WG_BIN"
command -v python3 >/dev/null || loud_skip "MISSING PYTHON3" "fixture setup requires python3"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_DIR WG_PROJECT_ROOT WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH

scratch=$(make_scratch)
export HOME="$scratch/home" WG_GLOBAL_DIR="$scratch/global" TMPDIR="$scratch/tmp"
export OPENROUTER_API_KEY=fake
fakebin="$scratch/fakebin"
mkdir -p "$HOME" "$WG_GLOBAL_DIR" "$TMPDIR" "$scratch/project" "$fakebin"
cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null
printf fresh-dispatch >fresh-dispatch.txt
sleep 60
SH
chmod +x "$fakebin/pi"
export PATH="$fakebin:$(dirname "$WG_BIN"):$PATH"
cd "$scratch/project"
git init -q -b main
git config user.email stale-owner@test.invalid
git config user.name 'Stale Owner Smoke'
printf 'baseline\n' >README
git add README && git commit -qm baseline
"$WG_BIN" init -m pi:openrouter:example/stale-owner --no-agency >/dev/null
G="$PWD/.wg"
"$WG_BIN" --dir "$G" config set dispatcher.poll_interval 1 >/dev/null
"$WG_BIN" --dir "$G" config set agency.auto_assign false >/dev/null
"$WG_BIN" --dir "$G" add 'stale owner convergence' --id stale-owner \
  --model pi:openrouter:example/stale-owner \
  -d $'## Validation\n- retained worktree dispatches' >/dev/null
"$WG_BIN" --dir "$G" publish stale-owner --only >/dev/null

# Seed a real Git-registered worktree carrying the private token and dirty WIP.
wt="$PWD/.wg-worktrees/agent-1"
mkdir -p "$PWD/.wg-worktrees"
git worktree add -q "$wt" -b wg/agent-1/stale-owner HEAD
printf 'valuable dirty evidence\n' >"$wt/valuable-wip.txt"
admin=$(git -C "$wt" rev-parse --absolute-git-dir)
owner="$admin/wg-spawn-owner.json"
base=$(git rev-parse HEAD)
python3 - "$owner" "$wt" "$base" <<'PY'
import json,os,sys
p,wt,base=sys.argv[1:]
json.dump({
  'schema':1,'token':'stale-private-token','agent_id':'agent-1',
  'task_id':'stale-owner','branch':'wg/agent-1/stale-owner',
  'path':os.path.realpath(wt),'base_oid':base
},open(p,'w'),indent=2)
PY
observer="$G/attempts/attempt-0-1/worktree-observer/state.json"
mkdir -p "$(dirname "$observer")"
cat >"$observer" <<'JSON'
{"projection":{"source":{"task_id":"stale-owner","generation":0,"attempt_id":"attempt-0-1","attempt_fence":1,"worktree_lease_epoch":1}},"retained":"observer evidence"}
JSON
mkdir -p "$G/service"
python3 - "$G/service/registry.json" "$wt" <<'PY'
import json,sys
p,wt=sys.argv[1:]
row={'id':'agent-1','pid':2147483000,'task_id':'stale-owner','executor':'shell',
     'started_at':'2020-01-01T00:00:00Z','last_heartbeat':'2020-01-01T00:00:00Z',
     'status':'working','output_file':'/retained/old-output.log',
     'completed_at':'2020-01-01T00:00:01Z','worktree_path':wt}
json.dump({'agents':{'agent-1':row},'next_agent_id':2},open(p,'w'),indent=2)
PY
owner_hash=$(sha256sum "$owner" | awk '{print $1}')
observer_hash=$(sha256sum "$observer" | awk '{print $1}')
wip_hash=$(sha256sum "$wt/valuable-wip.txt" | awk '{print $1}')

cleanup() { "$WG_BIN" --dir "$G" service stop --force --kill-agents >/dev/null 2>&1 || true; }
trap cleanup EXIT
"$WG_BIN" --dir "$G" service start --max-agents 1 --no-chat-agent --force >/dev/null

launched=false
for _ in $(seq 1 200); do
  [[ -f "$wt/fresh-dispatch.txt" ]] && { launched=true; break; }
  sleep 0.1
done
$launched || loud_fail "current attempt never dispatched from retained worktree: $(tail -100 "$G/service/daemon.log" 2>/dev/null || true)"

[[ "$(sha256sum "$owner" | awk '{print $1}')" == "$owner_hash" ]] || loud_fail 'stale owner token was edited or deleted'
[[ "$(sha256sum "$observer" | awk '{print $1}')" == "$observer_hash" ]] || loud_fail 'stale observer state was edited or deleted'
[[ "$(sha256sum "$wt/valuable-wip.txt" | awk '{print $1}')" == "$wip_hash" ]] || loud_fail 'dirty worktree evidence was edited or deleted'
[[ "$(cat "$wt/fresh-dispatch.txt")" == fresh-dispatch ]] || loud_fail 'new worker did not execute in retained worktree'

python3 - "$G/service/worktree-spawn-reclaims-v1.json" "$G/graph.jsonl" "$G/service/registry.json" "$G/service/worker-capabilities.json" <<'PY' || loud_fail 'reclaim/dispatch accounting drifted'
import json,sys
ledger=json.load(open(sys.argv[1]))
assert ledger['schema_version']==1,ledger
assert len(ledger['acknowledgements'])==1,ledger
ack=next(iter(ledger['acknowledgements'].values()))
assert ack['stale_agent_id']=='agent-1' and ack['task_id']=='stale-owner',ack
assert ack['owner_record_retained'] is True,ack
assert ack['observer_state_retained'] is True,ack
rows=[json.loads(x) for x in open(sys.argv[2]) if x.strip()]
task=next(x for x in rows if x.get('kind')=='task' and x.get('id')=='stale-owner')
assert task['status']=='in-progress',task
assert task['assigned']=='agent-2',task
assert task.get('spawn_failures',0)==0,task
assert task.get('dispatch_count',0)==1,task
assert task['lifecycle']['generation']==0,task['lifecycle']
assert task['lifecycle']['fence']==1,task['lifecycle']
registry=json.load(open(sys.argv[3]))
assert registry['agents']['agent-1']['worktree_path']==ack['worktree_path'],registry
assert registry['agents']['agent-2']['worktree_path']==ack['worktree_path'],registry
capabilities=json.load(open(sys.argv[4]))['capabilities']
current=next(v for v in capabilities.values() if v['agent_id']=='agent-2')
assert current['worktree_path']==ack['worktree_path'],current
assert not current['save_source']['worktree_identity_digest'].startswith('missing:'),current
PY

# The versioned pure incident trace is independently byte-stable.
fixture="$HERE/../../../formal/fixtures/daemon/v3/stale_worktree_spawn_owner.json"
python3 - "$fixture" "$scratch/trace.json" <<'PY'
import json,sys
json.dump(json.load(open(sys.argv[1]))['trace'],open(sys.argv[2],'w'),indent=2)
PY
"$WG_BIN" --dir "$G" service replay "$scratch/trace.json" --output "$scratch/one.json" >/dev/null
"$WG_BIN" --dir "$G" service replay "$scratch/trace.json" --output "$scratch/two.json" >/dev/null
cmp "$scratch/one.json" "$scratch/two.json" || loud_fail 'v3 stale-owner replay was not byte-identical'

echo 'PASS: proven-dead registered spawn owner reclaimed once; stale token/observer/dirty bytes retained; current attempt dispatched with no generation, fence, breaker, or falloff charge'
