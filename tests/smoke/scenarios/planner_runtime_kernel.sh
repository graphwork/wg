#!/usr/bin/env bash
# Candidate-binary proof for the production PlannerStore step-1 kernel:
# migrate exact legacy convergence scheduling state once, expose read-only
# status, and keep every planner authority byte stable across daemon restart.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$(command -v wg)}"
[[ -x "$WG_BIN" ]] || loud_fail "candidate WG_BIN is not executable: $WG_BIN"
unset WG_AGENT_ID WG_TASK_ID WG_EXECUTOR_TYPE WG_TIER WG_MODEL WG_WORKER_CAPABILITY WG_WORKER_IPC WG_WORKER_CONTROL_PROTOCOL WG_GRAPH_ID

scratch="$(make_scratch)"
project="$scratch/project"
home="$scratch/home"
mkdir -p "$project" "$home/.config/workgraph"
: >"$home/.config/workgraph/config.toml"
run_wg() {
  (cd "$project" && HOME="$home" XDG_CONFIG_HOME="$home/.config" "$WG_BIN" "$@")
}
run_wg init -m pi:openrouter:test/planner-runtime --no-agency >/dev/null
G="$(graph_dir_in "$project")" || loud_fail "missing graph"
run_wg --dir "$G" add "Planner migration goal" --id legacy-goal >/dev/null
run_wg --dir "$G" fail legacy-goal --reason "migration fixture hold" >/dev/null
cleanup() { run_wg --dir "$G" service stop --force >/dev/null 2>&1 || true; }
wait_planner_status() {
  local out="$1"
  for _ in $(seq 1 100); do
    if run_wg --json --dir "$G" service status >"$out" 2>/dev/null \
      && python3 - "$out" <<'PY' >/dev/null 2>&1
import json,sys
assert json.load(open(sys.argv[1])).get('planner_runtime') is not None
PY
    then
      return 0
    fi
    sleep .05
  done
  loud_fail "planner runtime status did not become ready: $(tail -40 "$G/service/daemon.log")"
}
trap cleanup EXIT

# Step 2 no longer creates or refreshes the retired convergence scheduler.
# Seed a real pre-cutover schema fixture before PlannerStore's one-time import.
legacy="$G/service/convergence-state.json"
cp "$HERE/../../fixtures/planner_runtime/convergence-state-v1.json" "$legacy"
python3 - "$legacy" <<'PY'
import json,sys
p=sys.argv[1]
d=json.load(open(p))
assert d['goals'], d
key=next(k for k in d['goals'] if k.startswith('legacy-goal#'))
g=d['goals'][key]
g['next_wake_at']='2031-02-03T04:05:06.123456789+00:00'
g['backoff']['failures_without_progress']=5
g['backoff']['base_seconds']=10
g['backoff']['cap_seconds']=640
g['backoff']['jitter_seed']='b3:legacy-jitter'
g['pending_action']=None
d['route_breakers']['pi|openrouter|b3:endpoint']={
  'route_id':'pi|openrouter|b3:endpoint', 'epoch':11,
  'state':'unavailable', 'consecutive_outages':4,
  'next_probe_at':'2031-02-03T04:07:06.222333444+00:00',
  'last_failure_marker':'2031-02-03T03:59:59+00:00'
}
open(p,'w').write(json.dumps(d,indent=2))
PY
rm -f "$G/service/decision-trace-v1.json" \
      "$G/service/planner-state-v1.json" \
      "$G/service/planner-effects-v1.json"

run_wg --dir "$G" service start --max-agents 0 --no-chat-agent --no-supervise --interval 60 >/dev/null
status1="$scratch/status1.json"
wait_planner_status "$status1"
python3 - "$status1" "$G" <<'PY' || loud_fail "planner status/import assertion failed"
import json,sys,os
s=json.load(open(sys.argv[1]))['planner_runtime']
assert s['schema_version']==5, s
assert s['last_sequence'] is None and s['next_sequence']==1, s
assert s['effects']=={}, s
legacy=s['legacy_convergence']
key=next(k for k in legacy['goals'] if k.startswith('legacy-goal:'))
g=legacy['goals'][key]
assert g['next_wake_at']=='2031-02-03T04:05:06.123456789+00:00', g
assert g['backoff']['failures_without_progress']==5, g
assert g['backoff']['jitter_seed']=='b3:legacy-jitter', g
route=legacy['routes']['pi|openrouter|b3:endpoint']
assert route['epoch']==11 and route['consecutive_outages']==4, route
assert route['next_probe_at']=='2031-02-03T04:07:06.222333444+00:00', route
# Imported AwaitDispatch timing is migration evidence only after step 2;
# planner-normalized dispatch observations own active deadlines.
assert s['earliest_deadline'] is None, s
for name in ('decision-trace-v1.json','planner-state-v1.json','planner-effects-v1.json'):
    assert os.path.isfile(os.path.join(sys.argv[2],'service',name)), name
PY
sha256sum "$G/service/decision-trace-v1.json" \
          "$G/service/planner-state-v1.json" \
          "$G/service/planner-effects-v1.json" >"$scratch/before.sha"
run_wg --dir "$G" service stop >/dev/null

# Restart must not re-import a now-changing legacy projection or rewrite any
# authoritative planner byte. Status itself is also non-mutating.
run_wg --dir "$G" service start --max-agents 0 --no-chat-agent --no-supervise --interval 60 >/dev/null
wait_planner_status "$scratch/status2.json"
sha256sum "$G/service/decision-trace-v1.json" \
          "$G/service/planner-state-v1.json" \
          "$G/service/planner-effects-v1.json" >"$scratch/after.sha"
cmp "$scratch/before.sha" "$scratch/after.sha" || loud_fail "planner bytes drifted across restart"
cmp -s "$status1" "$scratch/status2.json" || {
  # Whole service status contains uptime/pid. Compare only the planner projection.
  python3 - "$status1" "$scratch/status2.json" <<'PY' || loud_fail "planner status drifted across restart"
import json,sys
assert json.load(open(sys.argv[1]))['planner_runtime']==json.load(open(sys.argv[2]))['planner_runtime']
PY
}
run_wg --dir "$G" service stop >/dev/null
trap - EXIT

echo "PASS: production planner kernel imports legacy deadline/backoff/route state once and exposes byte-stable read-only status across restart"
