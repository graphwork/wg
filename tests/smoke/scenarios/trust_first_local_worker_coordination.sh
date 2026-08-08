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
exec bash worker.sh
SH
chmod +x "$scratch/bin/pi"
export PATH="$scratch/bin:$PATH" HOME="$home" XDG_CONFIG_HOME="$home/.config"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH WG_DIR

git -C "$project" init -q -b main
git -C "$project" config user.email trust-worker@test.invalid
git -C "$project" config user.name TrustWorker
cat >"$project/worker.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ ${WG_WORKER_CONTROL_MODE:-} == trusted ]] || { echo "expected trusted mode" >&2; exit 91; }
wg capabilities --json > capabilities.json
grep -q '"mode": "trusted"' capabilities.json
if wg service status > protected-service.out 2>&1; then
  echo "trusted worker unexpectedly gained service administration" >&2
  exit 92
fi
grep -q 'worker_control.admin_operation_refused' protected-service.out
if wg --dir /tmp list > protected-cross-graph.out 2>&1; then
  echo "trusted worker unexpectedly selected a different graph" >&2
  exit 93
fi
grep -q 'worker_control.graph_cli_cross_graph_refused' protected-cross-graph.out
wg show downstream --json > downstream-before.json
wg edit downstream --description $'Coordinated downstream description.\n\n## Validation\n- [ ] trust-first edit reached the graph'
wg add "Trusted local subtask" --id trusted-local-subtask --after "$WG_TASK_ID" --assign "$WG_AGENT_ID" \
  --description $'Created by a trusted local worker.\n\n## Validation\n- [ ] linked before downstream'
wg edit downstream --add-after trusted-local-subtask
wg reprioritize downstream critical
wg msg send downstream "trusted worker coordinated downstream metadata"
wg publish trusted-local-subtask --only
wg log "$WG_TASK_ID" "trust-first cross-task coordination complete"
printf 'trusted\n' > trust-first.txt
git add capabilities.json downstream-before.json trust-first.txt
git commit -qm 'trust-first worker evidence'
# Stay alive while the outer fixture audits exact attempt attribution.
sleep 120
SH
chmod +x "$project/worker.sh"
git -C "$project" add worker.sh
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
wgrun add "Local coordination worker" --id local-coordinator >/dev/null
wgrun add "Downstream implementation" --id downstream --after local-coordinator >/dev/null
wgrun publish local-coordinator --wcc >/dev/null
wgrun service start --max-agents 1 --no-coordinator-agent --no-supervise >/dev/null

worktree=""
for _ in $(seq 1 320); do
  worktree=$([[ -d "$project/.wg-worktrees" ]] && find "$project/.wg-worktrees" -mindepth 1 -maxdepth 1 -type d | head -1 || true)
  if [[ -n $worktree ]] && git -C "$worktree" show HEAD:trust-first.txt 2>/dev/null | grep -qx trusted; then
    break
  fi
  status=$(wgrun show local-coordinator --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)
  [[ $status == failed || $status == abandoned ]] && loud_fail "trusted worker terminal status: $status"
  sleep 0.25
done
[[ -n $worktree ]] || loud_fail "trusted worker worktree was not created"
git -C "$worktree" show HEAD:trust-first.txt | grep -qx trusted || loud_fail "trusted worker did not finish coordination"

downstream=$(wgrun show downstream --json)
printf '%s' "$downstream" | grep -q 'Coordinated downstream description' || loud_fail "downstream description not edited"
printf '%s' "$downstream" | grep -q 'trust-first edit reached the graph' || loud_fail "downstream validation not edited"
printf '%s' "$downstream" | grep -q 'trusted-local-subtask' || loud_fail "subtask not linked downstream"
printf '%s' "$downstream" | grep -Eq '"priority"[[:space:]]*:[[:space:]]*100' || loud_fail "downstream not reprioritized"
grep -q '"id":"trusted-local-subtask"' "$project/.wg/graph.jsonl" || loud_fail "trusted subtask not created"
grep -q 'trusted worker coordinated downstream metadata' "$project/.wg/messages/downstream.jsonl" || loud_fail "cross-task message absent"

registry="$project/.wg/service/worker-capabilities.json"
audit="$project/.wg/service/worker-capability-audit.jsonl"
read -r agent attempt fence < <(python3 - "$registry" <<'PY'
import json,sys
r=json.load(open(sys.argv[1]))
b=next(v for v in r['capabilities'].values() if v['task_id']=='local-coordinator')
print(b['agent_id'], b['attempt_id'], b['fence'])
PY
)
prompt="$project/.wg/agents/$agent/prompt.txt"
[[ -s $prompt ]] || loud_fail "spawned worker prompt missing"
grep -q 'Effective mode.*trusted' "$prompt" || loud_fail "startup prompt omitted effective trusted mode"
grep -q 'wg add' "$prompt" || loud_fail "cross-task creation instructions disappeared instead of receiving compatible authority"
grep -q 'wg edit' "$prompt" || loud_fail "cross-task edit instructions disappeared instead of receiving compatible authority"
python3 - "$audit" "$agent" "$attempt" "$fence" <<'PY'
import json,sys
path,agent,attempt,fence=sys.argv[1:]
fence=int(fence)
events=[json.loads(x) for x in open(path) if x.strip()]
graph=[e for e in events if e.get('operation')=='graph_cli' and e.get('outcome')=='allowed']
assert len(graph) >= 7, graph
assert all(e.get('agent_id')==agent and e.get('attempt_id')==attempt and e.get('fence')==fence for e in graph), graph
assert all(e.get('control_mode')=='trusted' for e in graph), graph
PY

show_source=$(wgrun show local-coordinator --json)
printf '%s' "$show_source" | grep -Eq '"worker_control_mode"[[:space:]]*:[[:space:]]*"trusted"' || loud_fail "show omitted effective trusted mode"
status=$(wgrun status --json)
printf '%s' "$status" | grep -Eq '"worker_control_mode"[[:space:]]*:[[:space:]]*"trusted"' || loud_fail "status omitted trusted default"

echo "PASS: default trusted local worker edited downstream metadata/validation, created and linked an assigned subtask, reprioritized, messaged, and every public graph mutation was audited to its exact actor/attempt/fence"
