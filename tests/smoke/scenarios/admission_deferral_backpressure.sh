#!/usr/bin/env bash
# Live credential-free daemon/terminal regression: one running build-heavy shell
# task holds the sole build slot while another stays Open under admission
# backpressure across more than five ticks, then runs exactly once when released.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
WG_BIN="${WG_BIN:-$(command -v wg)}"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON" "python3 required for graph assertions"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux required for live TUI admission visibility"

unset WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_REASONING WG_TIER
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
touch seed
git add seed
git commit -q -m seed
wg init --no-agency >/dev/null
cat >.wg/config.toml <<'EOF'
[agency]
auto_assign = false
auto_evaluate = false

[dispatcher]
poll_interval = 1
settling_delay_ms = 0
max_spawn_failures = 5
worktree_isolation = false

[dispatcher.resource_management]
disk_sentinel_enabled = false
max_build_agents = 1
disk_agent_heartbeat_seconds = 300
EOF
# The daemon requires an explicitly selected execution system even though both
# fixture tasks are inline shell work and never contact it.
wg config --local --model pi:openrouter:test/fake >/dev/null

runs="$project/runs.log"
release="$project/release-first"
second_release="$project/release-second"
first_started="$project/first-started"
wg add 'cargo build occupying the sole build-heavy slot' --id a-occupied-build --priority 100 \
  --exec "printf 'first-start\\n' >> '$runs'; touch '$first_started'; while [ ! -e '$release' ]; do sleep 0.1; done; printf 'first-done\\n' >> '$runs'" \
  --exec-mode shell >/dev/null
wg add 'cargo test deferred until build capacity frees' --id b-deferred-build --priority 10 \
  --exec "printf 'second-run\\n' >> '$runs'; while [ ! -e '$second_release' ]; do sleep 0.1; done; printf 'second-success\\n' >> '$runs'" --exec-mode shell >/dev/null
wg publish a-occupied-build --only >/dev/null
wg publish b-deferred-build --only >/dev/null

start_wg_daemon "$project" --max-agents 2 --no-chat-agent --interval 1

for _ in $(seq 1 120); do
  state=$(wg service status --json 2>/dev/null | python3 -c 'import json,sys; c=json.load(sys.stdin).get("coordinator",{}); print(c.get("dispatch_state",""), c.get("admission_deferred_tasks",0), c.get("admission_deferred_reason", ""))' 2>/dev/null || true)
  if [ -e "$first_started" ] && grep -q '^admission-deferred 1 build-heavy admission budget full (1/1)$' <<<"$state"; then
    break
  fi
  sleep 0.1
done
[ -e "$first_started" ] || loud_fail "occupying build never started: $(tail -100 .wg/service/daemon.log 2>/dev/null || true)"
grep -q '^admission-deferred 1 build-heavy admission budget full (1/1)$' <<<"${state:-}" \
  || loud_fail "service JSON did not expose build-budget deferral count/reason: ${state:-}; $(wg service status --json 2>&1)"
service_json=$(wg service status --json)
python3 - "$service_json" <<'PY'
import json,sys
c=json.loads(sys.argv[1])['coordinator']
assert c['build_heavy_active']==1,c
assert c['max_build_agents']==1,c
assert c['max_build_agents_source']=='explicit',c
assert c['max_build_agents_remediation_command']=='wg config set dispatcher.resource_management.max_build_agents inherit --local',c
assert c['disk_sentinel_enabled'] is False,c
assert c['projected_headroom_bytes'] is None,c
assert c['admission_deferred']==[{'task_id':'b-deferred-build','reason':'build-heavy admission budget full (1/1)'}],c
PY

human=$(wg service status)
grep -q 'Admission deferred: 1 ready task' <<<"$human" \
  || loud_fail "human service status omitted deferred count: $human"
grep -q 'Reason: build-heavy admission budget full (1/1)' <<<"$human" \
  || loud_fail "human service status omitted deferred reason: $human"
grep -q 'no spawn failure charged' <<<"$human" \
  || loud_fail "human service status did not distinguish backpressure: $human"
grep -q 'Build-heavy: 1/1 active (cap explicit).*wg config set dispatcher.resource_management.max_build_agents inherit --local' <<<"$human" \
  || loud_fail "human service status omitted cap source/remediation: $human"
grep -q 'Task b-deferred-build: build-heavy admission budget full (1/1)' <<<"$human" \
  || loud_fail "human service status omitted per-task reason: $human"
grep -q 'Disk sentinel: disabled.*projected headroom unavailable' <<<"$human" \
  || loud_fail "disabled disk sentinel fabricated headroom: $human"

status_json=$(wg status --json)
python3 - "$status_json" <<'PY'
import json,sys
c=json.loads(sys.argv[1])['coordinator']
assert c['build_heavy_active']==1,c
assert c['max_build_agents']==1,c
assert c['max_build_agents_source']=='explicit',c
assert c['max_build_agents_remediation_command']=='wg config set dispatcher.resource_management.max_build_agents inherit --local',c
assert c['disk_sentinel_enabled'] is False,c
assert c['admission_deferred'][0]['task_id']=='b-deferred-build',c
assert 'disk' not in json.loads(sys.argv[1]),'disabled projection must be unavailable'
PY
status_human=$(wg status)
grep -q 'b-deferred-build: build-heavy admission budget full (1/1)' <<<"$status_human" \
  || loud_fail "wg status omitted per-task admission reason: $status_human"
grep -q 'projected headroom unavailable' <<<"$status_human" \
  || loud_fail "wg status fabricated disabled disk headroom: $status_human"
if command -v worksgood >/dev/null 2>&1; then
  worksgood_status=$(worksgood --project "$project" status)
  grep -q 'Build-heavy: 1/1 active' <<<"$worksgood_status" \
    || loud_fail "worksgood operator status omitted build admission: $worksgood_status"
fi

# Drive the actual TUI. Select the last task, open Detail, and inspect both the
# dashboard pulse and selected-task waiting explanation.
session="wg-admission-ui-$$"
trap 'tmux kill-session -t "$session" 2>/dev/null || true; wg --dir "$project/.wg" service stop >/dev/null 2>&1 || true; rm -rf "$scratch"' EXIT
tmux new-session -d -s "$session" -x 180 -y 50 \
  "cd '$project' && env HOME='$HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$project/.wg' tui"
sleep 2
tmux send-keys -t "$session" End 1
sleep 1
tui_dump="$scratch/tui-admission.txt"
tmux capture-pane -p -t "$session" -S - >"$tui_dump" 2>&1 \
  || loud_fail "live TUI pane capture failed: $(cat "$tui_dump")"
grep -Eq 'B1/1|Build-heavy.*1.*1' "$tui_dump" \
  || loud_fail "TUI dashboard omitted build capacity: $(cat "$tui_dump")"
grep -q 'Admission waiting' "$tui_dump" \
  || loud_fail "TUI task inspector omitted admission waiting state: $(cat "$tui_dump")"
grep -q 'build-heavy admission budget full (1/1)' "$tui_dump" \
  || loud_fail "TUI task inspector omitted waiting reason: $(cat "$tui_dump")"
grep -q 'Build-heavy capacity: 1/1 (explicit)' "$tui_dump" \
  || loud_fail "TUI task inspector omitted active/max cap source: $(cat "$tui_dump")"
grep -q 'wg config set dispatcher.resource_management.max_build_agents' "$tui_dump" \
  && grep -q 'inherit --local' "$tui_dump" \
  || loud_fail "TUI task inspector omitted remediation: $(cat "$tui_dump")"
tmux kill-session -t "$session" 2>/dev/null || true

# Cross the configured five-failure threshold while capacity remains occupied.
# The source must remain untouched/Open, with one coalesced lifecycle event and
# no evaluation/FLIP satellite or provider failure charge.
sleep 6
python3 - <<'PY'
import json, pathlib
rows=[json.loads(line) for line in open('.wg/graph.jsonl') if line.strip()]
tasks={r['id']:r for r in rows if r.get('kind')=='task'}
t=tasks['b-deferred-build']
assert t['status']=='open',t
assert t.get('assigned') is None,t
assert t.get('spawn_failures',0)==0,t
assert t.get('retry_count',0)==0,t
assert t.get('dispatch_count',0)==0,t
assert '.evaluate-b-deferred-build' not in tasks,tasks.keys()
assert '.flip-b-deferred-build' not in tasks,tasks.keys()
audit=t.get('lifecycle',{}).get('audit',[])
deferrals=[e for e in audit if e.get('event_kind')=='admission-deferred']
assert len(deferrals)==1,deferrals
health=pathlib.Path('.wg/service/provider-health.json')
if health.exists():
    h=json.load(open(health))
    assert all(p.get('consecutive_failures',0)==0 for p in h.get('providers',{}).values()),h
PY
[ "$(grep -c "Deferring 'b-deferred-build': build-heavy admission budget full (1/1)" .wg/service/daemon.log || true)" -eq 1 ] \
  || loud_fail "identical admission logs were not coalesced: $(grep "b-deferred-build" .wg/service/daemon.log || true)"
[ "$(grep -c '^second-run$' "$runs" 2>/dev/null || true)" -eq 0 ] \
  || loud_fail "deferred build ran before capacity was released: $(cat "$runs")"

# Human releases capacity; no retry/edit/transition-helper call is made. The
# daemon's bounded tick notices the freed slot and runs the deferred task once.
touch "$release"
for _ in $(seq 1 160); do
  second_status=$(python3 - <<'PY'
import json
rows=[json.loads(line) for line in open('.wg/graph.jsonl') if line.strip()]
print([r for r in rows if r.get('kind')=='task' and r['id']=='b-deferred-build'][-1]['status'])
PY
)
  if [ "$second_status" = "in-progress" ] \
    && [ "$(grep -c '^second-run$' "$runs" 2>/dev/null || true)" -eq 1 ]; then
    break
  fi
  sleep 0.1
done
[ "${second_status:-}" = "in-progress" ] \
  || loud_fail "deferred build did not self-dispatch after release (status=${second_status:-unset}): $(tail -120 .wg/service/daemon.log 2>/dev/null || true)"
[ "$(grep -c '^second-run$' "$runs" 2>/dev/null || true)" -eq 1 ] \
  || loud_fail "deferred build did not run exactly once: $(cat "$runs" 2>/dev/null || true)"
python3 - <<'PY'
import json
rows=[json.loads(line) for line in open('.wg/graph.jsonl') if line.strip()]
t=[r for r in rows if r.get('kind')=='task' and r['id']=='b-deferred-build'][-1]
assert t['status']=='in-progress',t
assert t.get('spawn_failures',0)==0,t
assert t.get('retry_count',0)==0,t
assert t.get('dispatch_count',0) == 1,t
assert t.get('current_attempt') is not None or t.get('lifecycle',{}).get('current_attempt') is not None,t
PY
touch "$second_release"
for _ in $(seq 1 80); do
  [ "$(grep -c '^second-success$' "$runs" 2>/dev/null || true)" -eq 1 ] && break
  sleep 0.1
done
[ "$(grep -c '^second-success$' "$runs" 2>/dev/null || true)" -eq 1 ] \
  || loud_fail "deferred build command did not complete its successful work body: $(cat "$runs" 2>/dev/null || true)"
[ "$(grep -c '^second-run$' "$runs" 2>/dev/null || true)" -eq 1 ] \
  || loud_fail "deferred build command ran more than once after success: $(cat "$runs" 2>/dev/null || true)"

echo "PASS: live daemon reports admission backpressure, coalesces it beyond five ticks, and runs the deferred build work body successfully exactly once after capacity frees"
