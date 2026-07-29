#!/usr/bin/env bash
# Scenario: park_resume_live_owner_overlap
#
# Regression provenance (external older-cluster incident; no cluster paths or
# credentials belong in this fixture): Clean G2 Slurm 5111221 COMPLETED 0:0 in
# 3:01; Fault G2 Slurm 5111243 COMPLETED 0:0 in 2:02; recovery task
# complete-v21-current-rc-2n under agent-1688 reconciled preserved artifacts.
#
# A real fake-Pi worker parks through `wg wait` but deliberately remains alive
# and keeps its dirty isolated worktree. An operator immediately runs `wg
# resume`. The daemon must treat every retry blocked by that live owner as a
# breaker-neutral preparation deferral. Only after the test releases the old
# process may one same-generation continuation reuse the exact worktree and Pi
# session leaf. No credential, breaker repair, retry, or graph edit is used.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "graph assertions require python3"
command -v sha256sum >/dev/null 2>&1 || loud_skip "MISSING SHA256SUM" "evidence hashing requires sha256sum"
REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null)" \
  || loud_fail "cannot locate repository root"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg}"
[[ -x "$WG_BIN" ]] || loud_fail "current-source candidate missing: $WG_BIN (build once, then set WG_SMOKE_CANDIDATE_BIN)"
# Every subprocess, including the fake Pi child's `wg wait`, resolves this
# exact caller-built candidate before any PATH-global installation.
export PATH="$(dirname "$WG_BIN"):$PATH"
require_wg
candidate_commit=$(git -C "$REPO_ROOT" rev-parse HEAD)
candidate_version=$($WG_BIN --version 2>&1)
candidate_digest=$(sha256sum "$WG_BIN" | awk '{print $1}')
echo "CANDIDATE: commit=$candidate_commit sha256=$candidate_digest binary=$WG_BIN version=$candidate_version"

scratch="$(make_scratch)"
project="$scratch/project"
home="$scratch/home"
global="$scratch/global"
fakebin="$scratch/fakebin"
sync="$scratch/sync"
mkdir -p "$project" "$home" "$global" "$fakebin" "$sync"
export HOME="$home"
export WG_GLOBAL_DIR="$global"
export XDG_CONFIG_HOME="$home/.config"
export INITIAL_MARKER="$sync/initial-parked.tsv"
export RESUMED_MARKER="$sync/resumed.tsv"
export PHASE_LOCK="$sync/initial-phase"
export RELEASE_INITIAL="$sync/release-initial"
export FAKE_PIDS="$sync/fake-pids.tsv"
unset PI_SESSION_ID PI_SESSION_FILE PI_CODING_AGENT PI_MODEL PI_PROVIDER PI_REASONING_LEVEL

cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
# `wg wait` may complete the authoritative park and then report an auxiliary
# watchdog observation refusal while that same wrapper is still establishing
# its epoch. The graph assertion below is authority; do not let shell `-e`
# terminate the deliberately-live owner after the successful park transition.
set -uo pipefail
session_id= session_dir=
argv="$*"
while (($#)); do
  case "$1" in
    --session-id) session_id=${2:-}; shift 2 ;;
    --session-dir) session_dir=${2:-}; shift 2 ;;
    *) shift ;;
  esac
done
if mkdir "$PHASE_LOCK" 2>/dev/null; then
  printf 'initial\t%s\n' "$$" >>"$FAKE_PIDS"
  # The wrapper starts Pi and `pi-watchdog bootstrap` concurrently. Wait for
  # the durable exact PID/session authorization before parking; otherwise the
  # deliberately-fast fake could advance lifecycle revision first and turn
  # bootstrap into a stale-revision refusal (then the wrapper kills Pi).
  pi_state=${WG_WORKTREE_OBSERVER_STATE_DIR%/worktree-observer}/pi/state.json
  bootstrap_ready=false
  for _ in $(seq 1 500); do
    if [[ -s "$pi_state" ]] && python3 - "$pi_state" "$$" "$session_id" "$WG_DIR/graph.jsonl" <<'PY' >/dev/null 2>&1
import json,sys
s=json.load(open(sys.argv[1]))['state']; pid=int(sys.argv[2]); sid=sys.argv[3]
assert s['process']['pid']==pid,s['process']
assert s['session']['session_id']==sid,s['session']
assert s['classification']=='active' and not s['terminal'],s
t=next(r for r in map(json.loads,open(sys.argv[4])) if r.get('kind')=='task' and r.get('id')=='live-owner')
assert any(e.get('event_kind')=='pi-continuation-authorized' for e in t['lifecycle'].get('audit',[])),t['lifecycle']
PY
    then
      bootstrap_ready=true
      break
    fi
    sleep 0.01
  done
  $bootstrap_ready || exit 70
  printf '%s\n' 'uncommitted worktree WIP from the parked owner' >preserved-wip.txt
  PI_SESSION_ID="$session_id" wg wait "$WG_TASK_ID" --until message \
    --checkpoint 'live owner retained external-job WIP' >/dev/null
  printf '%s\t%s\t%s\t%s\t%s\n' \
    "${WG_ATTEMPT_ID:-missing-attempt}" "$session_id" "$session_dir" "$PWD" "$$" \
    >"$INITIAL_MARKER"
  # Deterministic overlap gate: remain alive after wg wait until the smoke test
  # has observed at least max_spawn_failures real dispatcher retries.
  while [[ ! -e "$RELEASE_INITIAL" ]]; do sleep 0.05; done
  exit 0
fi
printf 'resumed\t%s\n' "$$" >>"$FAKE_PIDS"
printf '%s\t%s\t%s\t%s\t%s\n' \
  "${WG_ATTEMPT_ID:-missing-attempt}" "$session_id" "$session_dir" "$PWD" "$argv" \
  >>"$RESUMED_MARKER"
# Keep the admitted continuation live so any duplicate launch is observable in
# lifecycle/registry state; the fixture cleanup owns termination.
while :; do sleep 1; done
SH
chmod +x "$fakebin/pi"
export PATH="$fakebin:$PATH"
printf 'commit=%s\nsha256=%s\nbinary=%s\nversion=%s\n' \
  "$candidate_commit" "$candidate_digest" "$WG_BIN" "$candidate_version" >"$sync/candidate-build.txt"

TEST_PASSED=0
cleanup_fake_pi() {
  if [[ -f "$FAKE_PIDS" ]]; then
    while IFS=$'\t' read -r _ pid; do
      [[ "$pid" =~ ^[0-9]+$ ]] && kill "$pid" >/dev/null 2>&1 || true
    done <"$FAKE_PIDS"
  fi
}
preserve_failure_evidence() {
  [[ "$TEST_PASSED" -eq 0 && -d "$scratch" ]] || return 0
  local root="${WG_SMOKE_FAILURE_EVIDENCE_ROOT:-${TMPDIR:-/tmp}/wg-smoke-failure-evidence}"
  local dest="$root/park-resume-live-owner-overlap.$$.${RANDOM}"
  mkdir -p "$dest"
  cp -a "$scratch/." "$dest/"
  printf 'Failure evidence preserved at %s\n' "$dest"
}
add_cleanup_hook cleanup_fake_pi
add_cleanup_hook preserve_failure_evidence

cd "$project"
git init -q
git config user.email park-resume@test.invalid
git config user.name 'Park Resume Live Owner Smoke'
printf 'base\n' >README.md
git add README.md
git commit -qm base
wg init --no-agency >/dev/null || loud_fail "wg init failed"
G="$project/.wg"
wg config --local --model pi:fake:fake-model --no-reload >/dev/null
wg config --auto-assign false --auto-evaluate false --flip-enabled false --no-reload >/dev/null
# Pin the incident breaker threshold explicitly. The overlap below is held for
# this many observed dispatcher spawn/preparation passes, not for a guessed
# wall-clock sleep.
wg config set dispatcher.max_spawn_failures 5 >/dev/null
wg config set dispatcher.poll_interval 1 >/dev/null
max_spawn_failures=$(wg config get dispatcher.max_spawn_failures | awk '/=/{print $3; exit}')
[[ "$max_spawn_failures" =~ ^[1-9][0-9]*$ ]] \
  || loud_fail "could not resolve positive max_spawn_failures: $max_spawn_failures"

wg add 'parked live owner overlap fixture' --id live-owner \
  --model pi:fake:fake-model --exec-mode full >/dev/null
wg publish live-owner --only >/dev/null
start_wg_daemon "$project" --max-agents 1 --no-chat-agent --interval 1 \
  || loud_fail "service start failed"
daemon_log="$G/service/daemon.log"
graph="$G/graph.jsonl"

# The initial attempt must be genuinely parked while both the fake Pi child and
# its registered wrapper remain alive. No fixed sleep participates in this
# synchronization.
parked=false
for _ in $(seq 1 300); do
  if [[ -s "$INITIAL_MARKER" ]] && python3 - "$graph" <<'PY' >/dev/null 2>&1
import json,sys
t=next(r for r in map(json.loads,open(sys.argv[1])) if r.get('kind')=='task' and r.get('id')=='live-owner')
assert t['status']=='waiting',t
assert t.get('assigned'),t
assert t['lifecycle']['current_attempt']['disposition']=='parked',t['lifecycle']
PY
  then
    parked=true
    break
  fi
  sleep 0.05
done
$parked || loud_fail "initial fake Pi did not reach live parked state: $(tail -80 "$daemon_log" 2>/dev/null || true)"
IFS=$'\t' read -r initial_attempt exact_session_id prior_session prior_worktree initial_fake_pid <"$INITIAL_MARKER"
prior_agent_dir=${prior_session%/pi-session}
prior_plan="$prior_agent_dir/pi-session-plan.json"
prior_metadata="$prior_agent_dir/metadata.json"
[[ -s "$prior_plan" && -s "$prior_metadata" ]] || loud_fail "initial Pi session/metadata attestation missing"
prior_wrapper_pid=$(python3 - "$prior_metadata" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['pid'])
PY
)
kill -0 "$initial_fake_pid" 2>/dev/null || loud_fail "parked fake Pi exited before operator resume"
kill -0 "$prior_wrapper_pid" 2>/dev/null || loud_fail "parked wrapper exited before operator resume"

python3 - "$graph" "$exact_session_id" "$prior_worktree" "$sync/baseline.json" <<'PY'
import json,sys
graph,sid,worktree,out=sys.argv[1:]
t=next(r for r in map(json.loads,open(graph)) if r.get('kind')=='task' and r.get('id')=='live-owner')
l=t['lifecycle']; cur=l['current_attempt']
assert t['status']=='waiting' and t.get('assigned'),t
assert cur['generation']==l['generation'],(cur,l)
assert cur['disposition']=='parked',cur
assert t.get('session_id')==sid,t
assert t.get('spawn_failures',0)==0 and t.get('last_spawn_failure_at') is None,t
json.dump({
  'generation':l['generation'], 'attempt_sequence':l['attempt_sequence'],
  'fence':l['fence'], 'attempt':cur, 'retry_count':t.get('retry_count',0),
  'assigned':t['assigned'], 'session_id':sid, 'worktree':worktree,
  'dispatch_count':t.get('dispatch_count',0)
},open(out,'w'))
PY

session_file=$(python3 - "$prior_plan" "$exact_session_id" "$prior_session" <<'PY'
import json,sys
p=json.load(open(sys.argv[1])); sid,session_dir=sys.argv[2:]
assert p['session_id']==sid,p
assert p['session_dir']==session_dir,p
assert p['resumed'] is False,p
assert p['canonical_leaf'].startswith('b3:'),p
print(p['session_file'])
PY
)
[[ -f "$session_file" ]] || loud_fail "attested Pi session file missing: $session_file"
cp "$prior_plan" "$sync/prior-plan.before.json"
prior_plan_hash=$(sha256sum "$prior_plan" | awk '{print $1}')
prior_session_hash=$(sha256sum "$session_file" | awk '{print $1}')
prior_wip_hash=$(sha256sum "$prior_worktree/preserved-wip.txt" | awk '{print $1}')
prior_wip_status=$(git -C "$prior_worktree" status --porcelain -- preserved-wip.txt)
[[ "$prior_wip_status" == '?? preserved-wip.txt' ]] || loud_fail "parked WIP was not dirty/untracked: $prior_wip_status"

# Real operator terminal flow while the parked owner is still live.
baseline_spawn_lines=$(grep -c "Spawning agent for: live-owner " "$daemon_log" 2>/dev/null || true)
wg resume live-owner --only >"$scratch/operator-resume.log" 2>&1 \
  || loud_fail "operator resume failed: $(cat "$scratch/operator-resume.log")"
kill -0 "$initial_fake_pid" 2>/dev/null || loud_fail "parked fake Pi died during operator resume"
kill -0 "$prior_wrapper_pid" 2>/dev/null || loud_fail "parked wrapper died during operator resume"

# Count real daemon attempts after the operator receipt. At every observed pass
# the old owner must still be alive and no competing fake Pi may launch.
overlap_ticks=0
for _ in $(seq 1 500); do
  kill -0 "$initial_fake_pid" 2>/dev/null || loud_fail "old fake Pi exited before overlap reached breaker threshold ($overlap_ticks/$max_spawn_failures)"
  kill -0 "$prior_wrapper_pid" 2>/dev/null || loud_fail "old wrapper exited before overlap reached breaker threshold ($overlap_ticks/$max_spawn_failures)"
  [[ ! -e "$RESUMED_MARKER" ]] || loud_fail "competing continuation launched while parked owner was live: $(cat "$RESUMED_MARKER")"
  current=$(grep -c "Spawning agent for: live-owner " "$daemon_log" 2>/dev/null || true)
  overlap_ticks=$((current - baseline_spawn_lines))
  if (( overlap_ticks >= max_spawn_failures )); then
    break
  fi
  sleep 0.05
done
(( overlap_ticks >= max_spawn_failures )) \
  || loud_fail "observed only $overlap_ticks/$max_spawn_failures live-owner dispatcher passes: $(tail -120 "$daemon_log")"

# Repeated live-owner preparation refusals are one coalesced, neutral evidence
# record. They may not reserve a competitor, change authority, or charge/surface
# the task breaker. Session and dirty-worktree bytes remain exact.
show_overlap=$(wg show live-owner 2>&1)
status_overlap=$(wg status 2>&1)
! grep -q 'Spawn circuit breaker TRIPPED' <<<"$show_overlap" \
  || loud_fail "breaker surfaced in wg show during live-owner overlap: $show_overlap"
! grep -Eq 'SPAWN BREAKER.*live-owner' <<<"$status_overlap" \
  || loud_fail "breaker surfaced in wg status during live-owner overlap: $status_overlap"
[[ "$prior_plan_hash" == "$(sha256sum "$prior_plan" | awk '{print $1}')" ]] \
  || loud_fail "initial Pi session plan changed during overlap"
[[ "$prior_session_hash" == "$(sha256sum "$session_file" | awk '{print $1}')" ]] \
  || loud_fail "attested Pi session leaf bytes changed during overlap"
[[ "$prior_wip_hash" == "$(sha256sum "$prior_worktree/preserved-wip.txt" | awk '{print $1}')" ]] \
  || loud_fail "dirty WIP changed during overlap"
[[ "$prior_wip_status" == "$(git -C "$prior_worktree" status --porcelain -- preserved-wip.txt)" ]] \
  || loud_fail "dirty WIP status changed during overlap"

python3 - "$graph" "$G/service/registry.json" "$sync/baseline.json" "$max_spawn_failures" <<'PY'
import json,os,sys
graph,registry_path,baseline_path,threshold=sys.argv[1:]
t=next(r for r in map(json.loads,open(graph)) if r.get('kind')=='task' and r.get('id')=='live-owner')
b=json.load(open(baseline_path)); l=t['lifecycle']; audit=l.get('audit',[])
assert t['status']=='open' and t.get('assigned') is None,t
assert l['generation']==b['generation'],(b,l)
assert l['attempt_sequence']==b['attempt_sequence'],(b,l)
assert l['fence']==b['fence'],(b,l)
assert l['current_attempt']==b['attempt'],(b,l['current_attempt'])
assert t.get('retry_count',0)==b['retry_count'],(b,t)
assert t.get('dispatch_count',0)==b['dispatch_count'],(b,t)
assert t.get('spawn_failures',0)==0,t
assert t.get('last_spawn_failure_at') is None,t
assert t.get('session_id')==b['session_id'],t
assert t.get('checkpoint')=='live owner retained external-job WIP',t
reserved=[e for e in audit if e.get('event_kind')=='attempt-reserved']
assert len(reserved)==1,reserved
assert not [e for e in audit if e.get('event_kind') in ('attempt-failed','attempt-lost','reservation-cancelled')],audit
wakes=[e for e in audit if e.get('event_kind')=='wait-satisfied' and e.get('reason_code')=='operator_resume']
assert len(wakes)==1,wakes
prep=[e for e in audit if e.get('reason_code')=='spawn_preparation_deferred']
assert len(prep)==1,prep
assert prep[0]['event_kind']=='admission-deferred',prep
logs=t.get('log',[])
assert sum(e.get('actor')=='spawn-preparation' for e in logs)==1,logs
assert not any('Spawn failed (attempt' in e.get('message','') for e in logs),logs
assert not any(e.get('actor')=='spawn-circuit-breaker' for e in logs),logs
registry=json.load(open(registry_path)).get('agents',{})
assert list(registry)==[b['assigned']],registry
owner=registry[b['assigned']]
assert owner['task_id']=='live-owner' and owner['status'].lower()=='parked',owner
assert owner.get('worktree_path')==b['worktree'],owner
PY
[[ "$(find "$G/agents" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')" == 1 ]] \
  || loud_fail "preparation deferrals left competing agent output: $(find "$G/agents" -mindepth 1 -maxdepth 2 -print)"
[[ "$(wc -l <"$FAKE_PIDS" | tr -d ' ')" == 1 ]] || loud_fail "competitor fake Pi launched during overlap: $(cat "$FAKE_PIDS")"
echo "PASS (1/2): $overlap_ticks live-owner dispatcher passes (threshold=$max_spawn_failures) were breaker-neutral; no competitor, authority/session/WIP stayed exact"

# Release only the old process. The daemon must naturally reconcile owner exit
# and admit exactly one continuation without wg retry or any graph mutation.
touch "$RELEASE_INITIAL"
resumed=false
for _ in $(seq 1 500); do
  if [[ -s "$RESUMED_MARKER" ]]; then
    resumed=true
    break
  fi
  sleep 0.05
done
$resumed || loud_fail "owner exit did not admit an exact continuation: $(tail -160 "$daemon_log")"

# The fake child can exit slightly before its wrapper is reaped; the new launch
# itself proves production observed the old registry PID as non-live. Still pin
# the externally visible process facts before accepting the continuation.
for _ in $(seq 1 200); do
  if ! kill -0 "$initial_fake_pid" 2>/dev/null && ! kill -0 "$prior_wrapper_pid" 2>/dev/null; then break; fi
  sleep 0.05
done
kill -0 "$initial_fake_pid" 2>/dev/null && loud_fail "initial fake Pi remained live after continuation launch"
kill -0 "$prior_wrapper_pid" 2>/dev/null && loud_fail "initial wrapper remained live after continuation launch"

# Wait for post-permit graph/registry accounting, rather than racing the fake
# Pi marker (which is written as soon as the handler begins).
settled=false
for _ in $(seq 1 300); do
  if python3 - "$graph" "$sync/baseline.json" <<'PY' >/dev/null 2>&1
import json,sys
t=next(r for r in map(json.loads,open(sys.argv[1])) if r.get('kind')=='task' and r.get('id')=='live-owner')
b=json.load(open(sys.argv[2])); l=t['lifecycle']
assert t['status']=='in-progress' and t.get('assigned') and t['assigned']!=b['assigned'],t
assert l['attempt_sequence']==b['attempt_sequence']+1,l
assert t.get('dispatch_count',0)==b['dispatch_count']+1,t
PY
  then
    settled=true
    break
  fi
  sleep 0.05
done
$settled || loud_fail "resumed attempt did not settle authoritative graph state: $(tail -120 "$daemon_log")"

IFS=$'\t' read -r resumed_attempt resumed_session_id resumed_session resumed_worktree resumed_argv <"$RESUMED_MARKER"
resumed_agent=$(python3 - "$graph" <<'PY'
import json,sys
print(next(r for r in map(json.loads,open(sys.argv[1])) if r.get('kind')=='task' and r.get('id')=='live-owner')['assigned'])
PY
)
resumed_plan="$G/agents/$resumed_agent/pi-session-plan.json"
[[ -s "$resumed_plan" ]] || loud_fail "resumed Pi session plan missing for $resumed_agent"

python3 - "$graph" "$G/lifecycle/events.jsonl" "$G/service/registry.json" \
  "$sync/baseline.json" "$resumed_agent" "$prior_worktree" <<'PY'
import json,sys
graph,events_path,registry_path,baseline_path,resumed_agent,worktree=sys.argv[1:]
t=next(r for r in map(json.loads,open(graph)) if r.get('kind')=='task' and r.get('id')=='live-owner')
b=json.load(open(baseline_path)); l=t['lifecycle']
events=[json.loads(x)['event'] for x in open(events_path) if x.strip()]
events=[e for e in events if e.get('task_id')=='live-owner']
assert t['status']=='in-progress' and t['assigned']==resumed_agent,t
assert resumed_agent!=b['assigned'],(b,resumed_agent)
assert l['generation']==b['generation'],(b,l)
assert l['attempt_sequence']==b['attempt_sequence']+1,(b,l)
assert l['current_attempt']['id']!=b['attempt']['id'],(b,l)
assert l['current_attempt']['generation']==b['generation'],l
assert l['current_attempt'].get('disposition') is None,l
assert t.get('retry_count',0)==b['retry_count'],(b,t)
assert t.get('dispatch_count',0)==b['dispatch_count']+1,(b,t)
assert t.get('spawn_failures',0)==0 and t.get('last_spawn_failure_at') is None,t
assert t.get('session_id')==b['session_id'],t
reserved=[e for e in events if e.get('event_kind')=='attempt-reserved']
assert len(reserved)==2,reserved
assert reserved[-1]['attempt_id']==l['current_attempt']['id'],(reserved,l)
assert reserved[-1]['generation']==b['generation'],reserved[-1]
assert not [e for e in events if e.get('event_kind') in ('attempt-failed','attempt-lost','generation-created','reservation-cancelled')],events
assert len([e for e in events if e.get('reason_code')=='spawn_preparation_deferred'])==1,events
assert len([e for e in events if e.get('event_kind')=='wait-satisfied' and e.get('reason_code')=='operator_resume'])==1,events
assert not any(e.get('actor')=='spawn-circuit-breaker' for e in t.get('log',[])),t.get('log',[])
registry=json.load(open(registry_path)).get('agents',{})
assert set(registry)=={b['assigned'],resumed_agent},registry
assert registry[resumed_agent]['task_id']=='live-owner',registry[resumed_agent]
assert registry[resumed_agent].get('worktree_path')==worktree,registry[resumed_agent]
assert registry[b['assigned']].get('worktree_path')==worktree,registry[b['assigned']]
PY

[[ "$resumed_session_id" == "$exact_session_id" ]] \
  || loud_fail "resume changed exact Pi session id: prior=$exact_session_id resumed=$resumed_session_id"
[[ "$resumed_session" == "$prior_session" ]] \
  || loud_fail "resume changed exact Pi session directory: prior=$prior_session resumed=$resumed_session"
[[ "$resumed_worktree" == "$prior_worktree" ]] \
  || loud_fail "resume changed isolated worktree: prior=$prior_worktree resumed=$resumed_worktree"
grep -q -- "--session-id $exact_session_id" <<<"$resumed_argv" \
  || loud_fail "resumed argv omitted exact session id: $resumed_argv"
grep -q -- "--session-dir $prior_session" <<<"$resumed_argv" \
  || loud_fail "resumed argv omitted exact session dir: $resumed_argv"
python3 - "$sync/prior-plan.before.json" "$resumed_plan" "$session_file" <<'PY'
import json,sys
prior,resumed=map(lambda p:json.load(open(p)),sys.argv[1:3]); session_file=sys.argv[3]
assert resumed['resumed'] is True,resumed
for key in ('session_id','session_dir','session_file','header_digest','canonical_leaf','canonical_prefix_len'):
    assert resumed[key]==prior[key],(key,prior,resumed)
assert resumed['session_file']==session_file,resumed
assert resumed['canonical_leaf'].startswith('b3:'),resumed
PY
[[ "$prior_plan_hash" == "$(sha256sum "$prior_plan" | awk '{print $1}')" ]] \
  || loud_fail "original Pi session plan changed after resume"
[[ "$prior_session_hash" == "$(sha256sum "$session_file" | awk '{print $1}')" ]] \
  || loud_fail "exact Pi session leaf bytes changed after resume"
[[ "$prior_wip_hash" == "$(sha256sum "$prior_worktree/preserved-wip.txt" | awk '{print $1}')" ]] \
  || loud_fail "preserved WIP changed after resume"
[[ "$prior_wip_status" == "$(git -C "$prior_worktree" status --porcelain -- preserved-wip.txt)" ]] \
  || loud_fail "preserved WIP lost dirty status after resume"
[[ "$(wc -l <"$RESUMED_MARKER" | tr -d ' ')" == 1 ]] \
  || loud_fail "more than one continuation handler launched: $(cat "$RESUMED_MARKER")"
[[ "$(wc -l <"$FAKE_PIDS" | tr -d ' ')" == 2 ]] \
  || loud_fail "expected initial + one resumed fake Pi only: $(cat "$FAKE_PIDS")"
[[ "$(find "$G/agents" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')" == 2 ]] \
  || loud_fail "expected exactly two attempt output directories: $(find "$G/agents" -mindepth 1 -maxdepth 2 -print)"

TEST_PASSED=1
echo 'PASS (2/2): owner exit admitted exactly one same-generation attempt in the same worktree with exact Pi session leaf + dirty WIP, without breaker repair or graph edit'
echo 'PASS: live parked-owner operator-resume overlap is breaker-neutral and exact-session retry-in-place is single-owner'
