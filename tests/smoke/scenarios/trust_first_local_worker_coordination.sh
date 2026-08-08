#!/usr/bin/env bash
# Trust-first local worker graph coordination through the public WG CLI.
set -euo pipefail
source "$(dirname "$0")/_helpers.sh"
: "${WG_BIN:?smoke harness must provide candidate WG_BIN}"
[[ -x $WG_BIN ]] || loud_fail "candidate WG_BIN is not executable: $WG_BIN"

scratch=$(mktemp -d "${TMPDIR:-/tmp}/wg-trust-worker.XXXXXX")
project="$scratch/project"
home="$scratch/home"
cleanup() {
  env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC \
    WG_DIR="$project/.wg" "$WG_BIN" service stop --force --kill-agents >/dev/null 2>&1 || true
  [[ ${WG_SMOKE_KEEP_TMP:-0} == 1 ]] || rm -rf "$scratch"
}
trap cleanup EXIT
mkdir -p "$project" "$home" "$scratch/bin"
ln -s "$WG_BIN" "$scratch/bin/wg"
cat >"$scratch/bin/pi" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
model=""
while (($#)); do
  case "$1" in --model) model="$2"; shift 2;; *) shift;; esac
done
if [[ $model == fake-review ]]; then
  cat >/dev/null || true
  printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"{\"verdict\":\"pass\",\"findings\":[]}"}],"provider":"test","model":"fake-review","stopReason":"stop","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}'
  exit 0
fi
bash "$(dirname "$0")/worker.sh"
printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"trusted quality pass completed through immutable review"}],"provider":"test","model":"fake-worker","stopReason":"stop","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}'
SH
chmod +x "$scratch/bin/pi"
export PATH="$scratch/bin:$PATH" HOME="$home" XDG_CONFIG_HOME="$home/.config"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH WG_DIR

git -C "$project" init -q -b main
git -C "$project" config user.email trust-worker@test.invalid
git -C "$project" config user.name TrustWorker
cat >"$scratch/bin/worker.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ ${WG_WORKER_CONTROL_MODE:-} == trusted ]] || { echo "expected trusted mode" >&2; exit 91; }
wg capabilities --json > capabilities.json
grep -q '"mode": "trusted"' capabilities.json
if wg service status > protected-service.out 2>&1; then
  echo "trusted worker unexpectedly gained service administration" >&2
  exit 92
fi
grep -q 'worker_control.operation_refused' protected-service.out
if wg --dir /tmp list > protected-cross-graph.out 2>&1; then
  echo "trusted worker unexpectedly selected a different graph" >&2
  exit 93
fi
grep -q 'worker_control.graph_cli_cross_graph_refused' protected-cross-graph.out
wg show downstream --json > downstream-before.json
wg edit downstream --description $'Coordinated downstream description.\n\n## Validation\n- [ ] trust-first edit reached the graph'
wg add "Trusted local subtask" --id trusted-local-subtask --after "$WG_TASK_ID" --assign "$WG_AGENT_ID" \
  --description $'Created and linked by a trusted local worker.\n\n## Validation\n- [ ] depends on its quality-pass parent'
wg reprioritize downstream critical
wg msg send downstream "trusted worker coordinated downstream metadata"
wg publish trusted-local-subtask --only
wg log "$WG_TASK_ID" "trust-first cross-task coordination complete"
printf 'trusted\n' > trust-first.txt
git add capabilities.json downstream-before.json trust-first.txt
git commit -qm 'trust-first worker evidence'
printf 'trusted quality-pass coordination completed\n' > summary.txt
printf 'cross-task edit/add/link/assign/priority/message assertions passed\n' > validation.log
wg completion-object validation.log --media-type text/plain --evidence-kind validation > evidence-ref.json
wg completion-manifest "$WG_TASK_ID" --summary summary.txt --git --evidence-ref evidence-ref.json > manifest.json
wg submit "$WG_TASK_ID" --manifest manifest.json --summary summary.txt >/dev/null
wg done "$WG_TASK_ID" >/dev/null
if wg edit downstream --description 'stale worker corrupted downstream' > stale-write.out 2>&1; then
  echo 'terminal worker unexpectedly retained graph-write authority' >&2
  exit 94
fi
grep -q 'worker_control.stale_capability' stale-write.out
if wg msg send downstream 'stale worker message must not append' > stale-message.out 2>&1; then
  echo 'terminal worker unexpectedly appended a message' >&2
  exit 95
fi
grep -q 'worker_control.stale_capability' stale-message.out
SH
chmod +x "$scratch/bin/worker.sh"
printf 'base\n' > "$project/README.md"
git -C "$project" add README.md
git -C "$project" commit -qm base
(
  cd "$project"
  "$WG_BIN" init --no-agency --route pi --model pi:openrouter:test/model >/dev/null
)
wgrun() {
  (cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC \
    WG_DIR="$project/.wg" "$WG_BIN" "$@")
}
# Omitted [worker_control] is the historical trust-first default.
wgrun config --local --auto-evaluate false --set-model reviewer pi:test:fake-review --set-model evaluator pi:test:fake-review --no-reload >/dev/null
wgrun add "Quality-pass local coordination worker" --id .quality-pass-local-coordinator >/dev/null
wgrun add "Downstream implementation" --id downstream --after .quality-pass-local-coordinator >/dev/null
wgrun publish .quality-pass-local-coordinator --wcc >/dev/null
wgrun service start --max-agents 1 --no-coordinator-agent --no-supervise >/dev/null

for _ in $(seq 1 320); do
  status=$(wgrun show .quality-pass-local-coordinator --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)
  [[ $status == done && -s "$project/stale-write.out" && -s "$project/stale-message.out" ]] && break
  [[ $status == failed || $status == abandoned ]] && loud_fail "trusted worker terminal status: $status"
  sleep 0.25
done
[[ ${status:-} == done && -s "$project/stale-write.out" && -s "$project/stale-message.out" ]] \
  || loud_fail "trusted quality-pass worker did not complete and prove its terminal fence"
wgrun service stop --force --kill-agents >/dev/null

downstream=$(wgrun show downstream --json)
printf '%s' "$downstream" | grep -Eq '"satisfied"[[:space:]]*:[[:space:]]*true' \
  || loud_fail "completed quality pass did not release downstream: $downstream"
ready=$(wgrun ready --json)
printf '%s' "$ready" | grep -q 'downstream' || loud_fail "released downstream was not ready"
printf '%s' "$downstream" | grep -q 'Coordinated downstream description' || loud_fail "downstream description not edited"
printf '%s' "$downstream" | grep -q 'trust-first edit reached the graph' || loud_fail "downstream validation not edited"
printf '%s' "$downstream" | grep -Eq '"priority"[[:space:]]*:[[:space:]]*100' || loud_fail "downstream not reprioritized"
child=$(wgrun show trusted-local-subtask --json)
printf '%s' "$child" | grep -q '.quality-pass-local-coordinator' || loud_fail "trusted subtask not linked to its quality-pass parent"
grep -q '"id":"trusted-local-subtask"' "$project/.wg/graph.jsonl" || loud_fail "trusted subtask not created"
grep -q 'trusted worker coordinated downstream metadata' "$project/.wg/messages/downstream.jsonl" || loud_fail "cross-task message absent"
if grep -q 'stale worker message must not append' "$project/.wg/messages/downstream.jsonl"; then
  loud_fail "stale trusted worker appended a message before its fence was rejected"
fi

registry="$project/.wg/service/worker-capabilities.json"
audit="$project/.wg/service/trusted-mutation-audit.jsonl"
read -r agent attempt fence < <(python3 - "$registry" <<'PY'
import json,sys
r=json.load(open(sys.argv[1]))
b=next(v for v in r['capabilities'].values() if v['task_id']=='.quality-pass-local-coordinator')
print(b['agent_id'], b['attempt_id'], b['fence'])
PY
)
printf '%s' "$child" | grep -q "$agent" || loud_fail "trusted subtask was not assigned to the quality-pass actor"
prompt="$project/.wg/agents/$agent/prompt.txt"
[[ -s $prompt ]] || loud_fail "spawned worker prompt missing"
grep -q 'Effective mode.*trusted' "$prompt" || loud_fail "startup prompt omitted effective trusted mode"
grep -q 'wg add' "$prompt" || loud_fail "cross-task creation instructions disappeared instead of receiving compatible authority"
grep -q 'wg edit' "$prompt" || loud_fail "cross-task edit instructions disappeared instead of receiving compatible authority"
python3 - "$audit" "$project/.wg/graph.jsonl" "$agent" "$attempt" "$fence" <<'PY'
import json,sys
path,graph_path,agent,attempt,fence=sys.argv[1:]
fence=int(fence)
events=[json.loads(x) for x in open(path) if x.strip()]
commits=[e for e in events if e.get('event')=='trusted_cli_graph_commit']
commands={e.get('command') for e in commits}
assert {'add','edit','reprioritize','msg','publish'} <= commands, commits
assert all(e.get('actor_id')==agent and e.get('attempt_id')==attempt and e.get('fence')==fence for e in commits), commits
tasks={row['id']:row for row in map(json.loads,open(graph_path)) if 'title' in row}
def audited(task, command):
    return any(
        entry.get('actor')==agent
        and f'command={command}' in entry.get('message','')
        and f'attempt={attempt}' in entry.get('message','')
        and f'fence={fence}' in entry.get('message','')
        for entry in tasks[task].get('log',[])
    )
def lifecycle_audited(task, command):
    return any(
        event.get('event_kind')=='trusted-graph-mutation'
        and event.get('actor_id')==agent
        and event.get('attempt_id')==attempt
        and event.get('fence')==fence
        and f':{command}:' in event.get('idempotency_key','')
        for event in tasks[task].get('lifecycle',{}).get('audit',[])
    )
assert audited('downstream','edit'), tasks['downstream'].get('log')
assert audited('downstream','reprioritize'), tasks['downstream'].get('log')
assert audited('downstream','msg'), tasks['downstream'].get('log')
assert audited('trusted-local-subtask','add'), tasks['trusted-local-subtask'].get('log')
assert audited('trusted-local-subtask','publish'), tasks['trusted-local-subtask'].get('log')
for task,command in [
    ('downstream','edit'),('downstream','reprioritize'),('downstream','msg'),
    ('trusted-local-subtask','add'),('trusted-local-subtask','publish')
]:
    assert lifecycle_audited(task,command), (task,command,tasks[task].get('lifecycle'))
PY

show_source=$(wgrun show .quality-pass-local-coordinator --json)
printf '%s' "$show_source" | grep -Eq '"worker_control_mode"[[:space:]]*:[[:space:]]*"trusted"' || loud_fail "show omitted effective trusted mode"
status=$(wgrun status --json)
printf '%s' "$status" | grep -Eq '"worker_control_mode"[[:space:]]*:[[:space:]]*"trusted"' || loud_fail "status omitted trusted default"

echo "PASS: default-trusted .quality-pass worker edited downstream metadata/validation, created/linked/assigned a subtask, reprioritized, messaged, completed through immutable review, released downstream, and audited every public graph mutation to its exact actor/attempt/fence"
