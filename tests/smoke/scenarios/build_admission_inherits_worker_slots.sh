#!/usr/bin/env bash
# Live terminal regression: absent build override follows effective worker slots,
# and both inherited/explicit hot reloads are visible without daemon restart.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON" "python3 required"

unset WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_REASONING WG_TIER
scratch=$(make_scratch)
project="$scratch/project"
home="$scratch/home"
mkdir -p "$project" "$home"
export HOME="$home" WG_GLOBAL_DIR="$home/.wg"
cd "$project"
git init -q -b main
git config user.email smoke@example.invalid
git config user.name 'WG Smoke'
touch seed && git add seed && git commit -q -m seed
wg init --no-agency >/dev/null
wg config init --local --bare >/dev/null
! grep -q 'max_build_agents' .wg/config.toml \
  || loud_fail "fresh wg init/config init baked in a serial max_build_agents throttle"
wg profile init-starters >/dev/null
! grep -R -q 'max_build_agents' "$HOME/.wg/profiles" \
  || loud_fail "fresh starter profile baked in a serial max_build_agents throttle"
wg config reset --route pi --yes >/dev/null
! grep -q 'max_build_agents' .wg/config.toml \
  || loud_fail "fresh config reset baked in a serial max_build_agents throttle"
setup_preview=$(wg setup --route pi --yes --model pi:openrouter:test/fake --dry-run)
! grep -q 'max_build_agents' <<<"$setup_preview" \
  || loud_fail "fresh setup preview baked in a serial max_build_agents throttle: $setup_preview"

# Remediation is qualified to the winning layer, including a global override.
wg config set dispatcher.resource_management.max_build_agents 1 --global --no-reload >/dev/null
global_status=$(wg status --json)
python3 - "$global_status" <<'PY'
import json,sys
c=json.loads(sys.argv[1])['coordinator']
assert c['max_build_agents_source']=='explicit',c
assert c['max_build_agents_remediation_command'].endswith('inherit --global'),c
PY
wg config set dispatcher.resource_management.max_build_agents inherit --global --no-reload >/dev/null
! grep -q 'max_build_agents' "$HOME/.wg/config.toml" \
  || loud_fail "global inheritance remediation did not remove the winning override"

cat >.wg/config.toml <<'EOF'
[agency]
auto_assign = false
auto_evaluate = false

[dispatcher]
max_agents = 3
poll_interval = 1
settling_delay_ms = 0
worktree_isolation = false

[dispatcher.resource_management]
disk_sentinel_enabled = false
EOF
wg config --local --model pi:openrouter:test/fake >/dev/null

# Effective config must report inheritance rather than an unexplained default 1.
get_json=$(wg --json config get dispatcher.resource_management.max_build_agents)
python3 - "$get_json" <<'PY'
import json,sys
x=json.loads(sys.argv[1]); assert x['value']==3,x; assert x['source']=='inherited-from-max-agents',x
PY

release="$project/release"
for n in 1 2 3; do
  wg add "cargo build inherited slot $n" --id "build-$n" --priority "$((100-n))" \
    --exec "touch '$project/started-$n'; while [ ! -e '$release' ]; do sleep 0.1; done" --exec-mode shell >/dev/null
  wg publish "build-$n" --only >/dev/null
done
start_wg_daemon "$project" --no-chat-agent --interval 1
trap 'wg --dir "$project/.wg" service stop >/dev/null 2>&1 || true; rm -rf "$scratch"' EXIT
for _ in $(seq 1 150); do
  [ -e started-1 ] && [ -e started-2 ] && [ -e started-3 ] && break
  sleep 0.1
done
[ -e started-1 ] && [ -e started-2 ] && [ -e started-3 ] \
  || loud_fail "three inherited build slots did not dispatch concurrently: $(wg service status --json 2>&1)"

j=$(wg service status --json)
python3 - "$j" <<'PY'
import json,sys
c=json.loads(sys.argv[1])['coordinator']
assert c['max_agents']==3,c
assert c['build_heavy_active']==3,c
assert c['max_build_agents']==3,c
assert c['max_build_agents_source']=='inherited-from-max-agents',c
assert c['max_build_agents_remediation_command']=='wg config set dispatcher.max_agents 4 --local',c
assert c['disk_sentinel_enabled'] is False,c
assert c['projected_headroom_bytes'] is None,c
PY

# A fourth build is actually held at inherited 3/3, then starts exactly once
# when max_agents hot-reloads to 4 without restarting the daemon.
wg add 'cargo build inherited hot slot' --id build-4 --priority 90 \
  --exec "touch '$project/started-4'; while [ ! -e '$release' ]; do sleep 0.1; done" --exec-mode shell >/dev/null
wg publish build-4 --only >/dev/null
sleep 2
[ ! -e started-4 ] || loud_fail "fourth inherited build bypassed the 3/3 worker/build cap"
python3 - <<'PY'
import json
rows=[json.loads(line) for line in open('.wg/graph.jsonl') if line.strip()]
t=[r for r in rows if r.get('kind')=='task' and r['id']=='build-4'][-1]
assert t.get('dispatch_count',0)==0,t
PY

pid=$(python3 -c 'import json; print(json.load(open(".wg/service/state.json"))["pid"])')
wg config set dispatcher.max_agents 4 >/dev/null
for _ in $(seq 1 80); do
  v=$(wg service status --json 2>/dev/null | python3 -c 'import json,sys; c=json.load(sys.stdin)["coordinator"]; print(c["max_agents"],c["max_build_agents"],c["max_build_agents_source"])' 2>/dev/null || true)
  [ "$v" = '4 4 inherited-from-max-agents' ] && break
  sleep 0.1
done
[ "${v:-}" = '4 4 inherited-from-max-agents' ] || loud_fail "inherited cap did not hot reload: ${v:-}"
for _ in $(seq 1 100); do
  [ -e started-4 ] && break
  sleep 0.1
done
[ -e started-4 ] || loud_fail "inherited hot reload reported 4 but did not admit build-4"
python3 - <<'PY'
import json
rows=[json.loads(line) for line in open('.wg/graph.jsonl') if line.strip()]
t=[r for r in rows if r.get('kind')=='task' and r['id']=='build-4'][-1]
assert t.get('dispatch_count',0)==1,t
PY

wg config set dispatcher.resource_management.max_build_agents 1 >/dev/null
for _ in $(seq 1 80); do
  v=$(wg service status --json 2>/dev/null | python3 -c 'import json,sys; c=json.load(sys.stdin)["coordinator"]; print(c["max_build_agents"],c["max_build_agents_source"])' 2>/dev/null || true)
  [ "$v" = '1 explicit' ] && break
  sleep 0.1
done
[ "${v:-}" = '1 explicit' ] || loud_fail "explicit cap did not hot reload: ${v:-}"
# Leave one ordinary worker slot open so the next task reaches the independent
# explicit build-heavy gate rather than stopping at the general worker cap.
wg config set dispatcher.max_agents 5 >/dev/null
for _ in $(seq 1 80); do
  v=$(wg service status --json 2>/dev/null | python3 -c 'import json,sys; c=json.load(sys.stdin)["coordinator"]; print(c["max_agents"],c["max_build_agents"],c["max_build_agents_source"])' 2>/dev/null || true)
  [ "$v" = '5 1 explicit' ] && break
  sleep 0.1
done
[ "${v:-}" = '5 1 explicit' ] || loud_fail "explicit cap did not remain independent while worker slots increased: ${v:-}"

# A fifth build is held by the explicit cap, then starts exactly once when the
# explicit override hot-reloads from 1 to 5.
wg add 'cargo build explicit hot slot' --id build-5 --priority 80 \
  --exec "touch '$project/started-5'; while [ ! -e '$release' ]; do sleep 0.1; done" --exec-mode shell >/dev/null
wg publish build-5 --only >/dev/null
for _ in $(seq 1 100); do
  deferred=$(wg service status --json 2>/dev/null | python3 -c 'import json,sys; c=json.load(sys.stdin)["coordinator"]; print(c["admission_deferred_tasks"], c.get("admission_deferred",[{}])[0].get("task_id",""))' 2>/dev/null || true)
  [ "$deferred" = '1 build-5' ] && break
  sleep 0.1
done
[ "${deferred:-}" = '1 build-5' ] || loud_fail "fifth build was not deferred by explicit cap: ${deferred:-}"
python3 - <<'PY'
import json
rows=[json.loads(line) for line in open('.wg/graph.jsonl') if line.strip()]
t=[r for r in rows if r.get('kind')=='task' and r['id']=='build-5'][-1]
assert t.get('dispatch_count',0)==0,t
PY

lint=$(wg config lint --local)
grep -q 'explicit build-heavy throttle' <<<"$lint" || loud_fail "lint omitted legacy-safe throttle warning: $lint"
grep -q 'wg config set dispatcher.resource_management.max_build_agents inherit' <<<"$lint" \
  || loud_fail "lint omitted exact opt-out command: $lint"
wg config set dispatcher.resource_management.max_build_agents 5 >/dev/null
for _ in $(seq 1 100); do
  [ -e started-5 ] && break
  sleep 0.1
done
[ -e started-5 ] || loud_fail "explicit hot reload reported 5 but did not admit build-5"
python3 - <<'PY'
import json
rows=[json.loads(line) for line in open('.wg/graph.jsonl') if line.strip()]
t=[r for r in rows if r.get('kind')=='task' and r['id']=='build-5'][-1]
assert t.get('dispatch_count',0)==1,t
PY
[ "$pid" = "$(python3 -c 'import json; print(json.load(open(".wg/service/state.json"))["pid"])')" ] \
  || loud_fail "capacity hot reload restarted daemon"

# An equal-valued explicit override still pins future capacity and therefore
# remains visible with the same exact inheritance restoration command.
wg config set dispatcher.resource_management.max_build_agents 5 >/dev/null
lint=$(wg config lint --local)
grep -q 'explicit cap pins build-heavy capacity' <<<"$lint" \
  || loud_fail "lint hid equal-valued explicit override: $lint"
grep -q 'max_build_agents inherit' <<<"$lint" \
  || loud_fail "lint omitted equal-cap inheritance remediation: $lint"

# The advertised command removes the key and hot-reloads back to inheritance.
wg config set dispatcher.resource_management.max_build_agents inherit >/dev/null
for _ in $(seq 1 80); do
  v=$(wg service status --json 2>/dev/null | python3 -c 'import json,sys; c=json.load(sys.stdin)["coordinator"]; print(c["max_build_agents"],c["max_build_agents_source"])' 2>/dev/null || true)
  [ "$v" = '5 inherited-from-max-agents' ] && break
  sleep 0.1
done
[ "${v:-}" = '5 inherited-from-max-agents' ] || loud_fail "inherit remediation did not hot reload: ${v:-}"
! grep -q 'max_build_agents' .wg/config.toml \
  || loud_fail "inherit remediation did not remove the explicit key"

touch "$release"
wg service stop >/dev/null
for _ in $(seq 1 50); do
  kill -0 "$pid" 2>/dev/null || break
  sleep 0.1
done
kill -0 "$pid" 2>/dev/null && loud_fail "daemon did not stop before stale-state status check"
wg config set dispatcher.max_agents 6 --no-reload >/dev/null
payload=$(wg status --json)
python3 - "$payload" <<'PY'
import json,sys
c=json.loads(sys.argv[1])['coordinator']
assert c['max_agents']==6,c
assert c['max_build_agents']==6,c
assert c['max_build_agents_source']=='inherited-from-max-agents',c
assert c['max_build_agents_remediation_command']=='wg config set dispatcher.max_agents 7 --local',c
assert c['projected_headroom_bytes'] is None,c
assert c['admission_deferred_tasks']==0,c
assert c.get('admission_deferred',[])==[],c
PY

echo "PASS: inherited/explicit hot reloads enforce exact-once admission; generators, remediation, and stopped status pass"
