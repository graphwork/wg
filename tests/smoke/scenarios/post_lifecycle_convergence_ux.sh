#!/usr/bin/env bash
# Candidate-binary proof for planner-authoritative dispatch and route health.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
unset WG_AGENT_ID WG_DIR WG_PROJECT_ROOT WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH WG_TASK_ID \
  WG_EXECUTOR_TYPE WG_TIER WG_MODEL WG_WORKER_CAPABILITY WG_WORKER_IPC \
  WG_WORKER_CONTROL_PROTOCOL WG_GRAPH_ID

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME" "$scratch/bin"
cd "$scratch"
git init -q
git config user.email smoke@example.invalid
git config user.name smoke
echo seed >README.md
git add README.md
git commit -qm seed

wg init --no-agency >init.log 2>&1 || loud_fail "wg init failed: $(cat init.log)"
wg_dir="$scratch/.wg"
wg --dir "$wg_dir" config --local -m pi:openrouter:example/convergence-smoke --no-reload \
  >config.log 2>&1 || loud_fail "route config failed: $(cat config.log)"
for kv in \
  dispatcher.poll_interval=60 \
  dispatcher.settling_delay_ms=20 \
  agency.auto_assign=false \
  dispatcher.convergence.base_seconds=1 \
  dispatcher.convergence.cap_seconds=2 \
  dispatcher.convergence.route_probe_base_seconds=1 \
  dispatcher.convergence.route_probe_cap_seconds=2 \
  dispatcher.convergence.action_lease_seconds=1 \
  dispatcher.convergence.jitter_divisor=1000000
do
  key=${kv%%=*}; value=${kv#*=}
  wg --dir "$wg_dir" config set "$key" "$value" >>config.log 2>&1 \
    || loud_fail "config set $kv failed: $(cat config.log)"
done

# A real waiting task is normalized before the no-ready early return. Its
# future deadline is PlannerStore's event-loop deadline and survives restart.
future_at=$(date -u -d '+5 minutes' +%Y-%m-%dT%H:%M:%SZ)
wg --dir "$wg_dir" add "future planner wait" --id future-wait \
  -d "dependency/readiness deadline persistence" --not-before "$future_at" >/dev/null
wg --dir "$wg_dir" publish future-wait --only >/dev/null

planner="$wg_dir/service/planner-state-v1.json"
trace="$wg_dir/service/decision-trace-v1.json"
start_wg_daemon "$scratch" --no-chat-agent --force --max-agents 1
ready=false
for _ in $(seq 1 120); do
  if [[ -f "$planner" ]] && python3 - "$planner" <<'PY' >/dev/null 2>&1
import json, sys
s=json.load(open(sys.argv[1]))
assert s['schema_version'] == 5
rows=list(s.get('tasks',{}).values())
row=next(r for r in rows if r['key']['task_id']=='future-wait')
wait=row['external_wait']
assert wait['kind'] in ('correlated_message','dependency_change')
assert wait.get('deadline',0) > s['logical_time']
PY
  then ready=true; break; fi
  sleep 0.1
done
$ready || loud_fail "planner readiness deadline not observed: $(cat "$planner" 2>/dev/null || true)"
python3 - "$planner" "$scratch/before.json" <<'PY'
import json,sys
s=json.load(open(sys.argv[1])); row=next(r for r in s['tasks'].values() if r['key']['task_id']=='future-wait')
json.dump({'deadline':row['external_wait']['deadline']},open(sys.argv[2],'w'))
PY
wg --dir "$wg_dir" service stop --force --kill-agents >/dev/null 2>&1 || true
start_wg_daemon "$scratch" --no-chat-agent --force --max-agents 1
sleep 0.6
python3 - "$planner" "$scratch/before.json" <<'PY'
import json,sys
s=json.load(open(sys.argv[1])); before=json.load(open(sys.argv[2]))
row=next(r for r in s['tasks'].values() if r['key']['task_id']=='future-wait')
assert row['external_wait']['deadline']==before['deadline'], (row,before)
PY
wg --dir "$wg_dir" service stop --force --kill-agents >/dev/null 2>&1 || true
echo "PASS (1/3): dependency/readiness deadline is a restart-stable planner forward class"

# Fake Pi keeps the one admitted ordinary-task probe alive. No credential or
# network is used, and the exact task model remains unchanged.
cat >"$scratch/bin/pi" <<'SH'
#!/bin/sh
sleep 30
SH
chmod +x "$scratch/bin/pi"
export PATH="$scratch/bin:$PATH"
for id in route-a route-b; do
  wg --dir "$wg_dir" add "route probe candidate $id" --id "$id" \
    -d "Same exact Pi/OpenRouter route; only one may probe." --timeout 45s >/dev/null
  wg --dir "$wg_dir" publish "$id" --only >/dev/null
done
cat >"$wg_dir/service/provider_health.json" <<JSON
{
  "providers": {
    "pi|openrouter|self-authenticated": {
      "provider_id": "pi|openrouter|self-authenticated",
      "consecutive_failures": 3,
      "last_failure_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
      "last_error": "credential-free seeded outage",
      "is_paused": true,
      "paused_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
      "pause_reason": "seeded route outage"
    }
  },
  "service_paused": true,
  "pause_reason": "legacy global state must not control dispatch",
  "paused_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "auto_resume_at": null
}
JSON
start_wg_daemon "$scratch" --no-chat-agent --force --max-agents 2
probe=false
for _ in $(seq 1 180); do
  if python3 - "$planner" "$wg_dir/agents/registry.json" <<'PY' >/dev/null 2>&1
import json, sys
s=json.load(open(sys.argv[1])); b=s['routes']['pi|openrouter|self-authenticated']
assert b['state']=='probing'
lease=b['probe_lease']
assert lease['task_id'] in ('route-a','route-b')
assert lease.get('spawned') is True
assert 'expires_at' not in lease
assert len([x for x in s['routes'].values() if x.get('probe_lease')])==1
assert len(s['effects'])==1, s['effects']
binding=next(iter(s['effects'].values()))['binding']
assert binding['route_id']=='pi|openrouter|self-authenticated', binding
assert binding['plan_id']==next(r['effect_binding']['plan_id'] for r in s['tasks'].values() if r['key']['task_id'] in ('route-a','route-b')), binding
PY
  then probe=true; break; fi
  sleep 0.15
done
$probe || loud_fail "one exact-route planner probe lease was not observed: planner=$(cat "$planner" 2>/dev/null || true) daemon=$(tail -80 "$wg_dir/service/daemon.log" 2>/dev/null || true) route=$(wg --dir "$wg_dir" show route-a 2>&1 || true)"
echo "PASS (2/3): one exact-route/model planner probe lease, no fallback"

# Outlive the configured one-second pre-execution lease. A successfully spawned
# probe is receipt-bound, not timer-expired, so no second task storms the route.
sleep 3
python3 - "$planner" "$wg_dir/graph.jsonl" <<'PY'
import json,sys
s=json.load(open(sys.argv[1])); b=s['routes']['pi|openrouter|self-authenticated']
assert b['state']=='probing'
assert b['probe_lease']['spawned'] is True
assert 'expires_at' not in b['probe_lease']
assert len(s['effects'])==1, s['effects']
rows=[json.loads(line) for line in open(sys.argv[2]) if line.strip()]
ids=[r.get('id','') for r in rows]
for prefix in ('.daemon-','.supervisor-','.probe-','.merge-','.cleanup-'):
    assert not any(i.startswith(prefix) for i in ids), (prefix,ids)
PY
status=$(wg --dir "$wg_dir" service status --json)
python3 - <<'PY' "$status"
import json,sys
s=json.loads(sys.argv[1])
assert not s.get('coordinator',{}).get('paused',False), s
PY
wg --dir "$wg_dir" service stop --force --kill-agents >/dev/null 2>&1 || true
echo "PASS (3/3): long outage has no probe storm, global pause, fallback, or controller task"

echo "PASS: planner-authoritative dispatch/readiness/route convergence"
