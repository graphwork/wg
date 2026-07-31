#!/usr/bin/env bash
# Deterministic real-daemon/operator regression for reopen vs live Pi owners.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "graph assertions require python3"
REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg}"
[[ -x "$WG_BIN" ]] || loud_fail "candidate missing: $WG_BIN"
export PATH="$(dirname "$WG_BIN"):$PATH"

scratch="$(make_scratch)"
project="$scratch/project"
home="$scratch/home"
fakebin="$scratch/fakebin"
sync="$scratch/sync"
mkdir -p "$project" "$home" "$fakebin" "$sync"
export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$scratch/global" REOPEN_SYNC="$sync"
unset PI_SESSION_ID PI_SESSION_FILE PI_CODING_AGENT PI_MODEL PI_PROVIDER PI_REASONING_LEVEL
unset WG_TASK_ID WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER WG_WORKTREE_PATH WG_BRANCH
unset WG_DIR WG_PROJECT_ROOT WG_REASONING WG_SPAWN_EPOCH WG_SPAWN_RUN_ID WG_TASK_TIMEOUT_SECS WG_WORKTREE_ACTIVE WG_WORKTREE_OBSERVER_STATE_DIR

cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -uo pipefail
# Every first owner survives the reopen command's polite SIGTERM so all holds
# are observable, then releases only at the deterministic restart boundary.
term_requested=0
trap 'term_requested=1' TERM
session_id= session_dir=
while (($#)); do
  case "$1" in
    --session-id) session_id=${2:-}; shift 2 ;;
    --session-dir) session_dir=${2:-}; shift 2 ;;
    *) shift ;;
  esac
done
prefix="$REOPEN_SYNC/$WG_TASK_ID"
if mkdir "$prefix.first-owner" 2>/dev/null; then
  pi_state=${WG_WORKTREE_OBSERVER_STATE_DIR%/worktree-observer}/pi/state.json
  ready=false
  for _ in $(seq 1 500); do
    if [[ -s "$pi_state" ]] && python3 - "$pi_state" "$$" "$session_id" <<'PY' >/dev/null 2>&1
import json,sys
s=json.load(open(sys.argv[1]))['state']
assert s['process']['pid']==int(sys.argv[2]),s['process']
assert s['session']['session_id']==sys.argv[3],s['session']
assert s['classification']=='active' and not s['terminal'],s
PY
    then ready=true; break; fi
    sleep 0.01
  done
  $ready || exit 70
  printf 'retained WIP from exact old owner %s\n' "$WG_TASK_ID" >reopen-wip.txt
  printf '%s\t%s\t%s\t%s\t%s\n' "${WG_ATTEMPT_ID:-missing}" "$session_id" "$session_dir" "$PWD" "$$" >"$prefix.first.tsv"
  while [[ ! -e "$prefix.release-first" || "$term_requested" != 1 ]]; do sleep 0.1; done
  exit 0
fi
printf '%s\t%s\t%s\t%s\t%s\n' "${WG_ATTEMPT_ID:-missing}" "$session_id" "$session_dir" "$PWD" "$$" >"$prefix.second.tsv"
while [[ ! -e "$prefix.release-second" ]]; do sleep 0.1; done
exit 0
SH
chmod +x "$fakebin/pi"
export PATH="$fakebin:$PATH"

tasks=(retry-owner requeue-owner reset-owner stale-owner)
cleanup_all() {
  for task in "${tasks[@]}"; do touch "$sync/$task.release-second" 2>/dev/null || true; done
  (cd "$project" && "$WG_BIN" service stop >/dev/null 2>&1) || true
}
add_cleanup_hook cleanup_all

cd "$project"
git init -q
git config user.email reopen-owner@test.invalid
git config user.name 'Reopen Owner Smoke'
printf 'base\n' >README.md
git add README.md
git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
"$WG_BIN" config --local --model pi:fake:fake-model --no-reload >/dev/null
"$WG_BIN" config --auto-assign false --auto-evaluate false --flip-enabled false --no-reload >/dev/null
"$WG_BIN" config set dispatcher.poll_interval 1 >/dev/null
for task in "${tasks[@]}"; do
  "$WG_BIN" add "live Pi $task" --id "$task" --model pi:fake:fake-model --exec-mode full >/dev/null
  "$WG_BIN" publish "$task" --only >/dev/null
done
start_wg_daemon "$project" --max-agents 4 --no-chat-agent --interval 1

for task in "${tasks[@]}"; do
  for _ in $(seq 1 600); do [[ -s "$sync/$task.first.tsv" ]] && break; sleep 0.02; done
  [[ -s "$sync/$task.first.tsv" ]] || loud_fail "$task first Pi owner never became authoritative: $(tail -100 .wg/service/daemon.log 2>/dev/null || true)"
done

declare -A old_attempt old_session old_session_dir old_worktree old_pi_pid old_agent_dir old_wrapper_pid
for task in "${tasks[@]}"; do
  IFS=$'\t' read -r old_attempt[$task] old_session[$task] old_session_dir[$task] old_worktree[$task] old_pi_pid[$task] <"$sync/$task.first.tsv"
  old_agent_dir[$task]=${old_session_dir[$task]%/pi-session}
  old_wrapper_pid[$task]=$(python3 - "${old_agent_dir[$task]}/metadata.json" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['pid'])
PY
)
  kill -0 "${old_pi_pid[$task]}" 2>/dev/null || loud_fail "$task old Pi owner not live"
  kill -0 "${old_wrapper_pid[$task]}" 2>/dev/null || loud_fail "$task old wrapper not live"
done
old_wip_hash=$(sha256sum "${old_worktree[retry-owner]}/reopen-wip.txt" | awk '{print $1}')
old_session_hash=$(find "${old_session_dir[retry-owner]}" -type f -name '*.jsonl' -print0 | sort -z | xargs -0 sha256sum | sha256sum | awk '{print $1}')

# Freeze the daemon at the boundary and exercise all supported reopen surfaces:
# abandon→retry, direct triage requeue, and abandon→reset. TUI retry dispatches
# the same command effect; HUD rendering has focused unit coverage.
"$WG_BIN" service stop >/dev/null 2>&1 || true
for task in "${tasks[@]}"; do kill -0 "${old_pi_pid[$task]}" 2>/dev/null || loud_fail "service stop killed $task detached Pi owner"; done
"$WG_BIN" abandon retry-owner --reason 'operator superseded live attempt' >/dev/null
retry_output=$("$WG_BIN" retry retry-owner)
requeue_output=$("$WG_BIN" requeue requeue-owner --reason 'deterministic triage retry')
"$WG_BIN" abandon reset-owner --reason 'operator reset live attempt' >/dev/null
reset_output=$("$WG_BIN" reset reset-owner --yes 2>&1)
stale_output=$("$WG_BIN" retry stale-owner)
for output in "$retry_output" "$requeue_output" "$reset_output" "$stale_output"; do
  grep -q 'waiting-for-owner-release' <<<"$output" || loud_fail "operator reopen did not report owner hold: $output"
done
python3 - .wg/graph.jsonl <<'PY'
import json,sys
tasks={x['id']:x for x in map(json.loads,open(sys.argv[1])) if x.get('kind')=='task'}
for tid,operation,status in [('retry-owner','retry','abandoned'),('requeue-owner','requeue','in-progress'),('reset-owner','reset','abandoned'),('stale-owner','retry','in-progress')]:
    t=tasks[tid]; i=t['lifecycle']['reopen_intent']
    assert t['status']==status,t
    assert t['lifecycle']['generation']==0,t['lifecycle']
    assert i['operation']==operation and i['source_attempt_id']=='attempt-0-1',i
    assert t.get('spawn_failures',0)==0,t
PY

for task in "${tasks[@]}"; do
  show=$("$WG_BIN" show "$task")
  grep -q 'waiting-for-owner-release' <<<"$show" || loud_fail "wg show hid $task owner-release hold: $show"
  [[ ! -e "$sync/$task.second.tsv" ]] || loud_fail "$task competing Pi process launched while old owner lived"
  kill -0 "${old_pi_pid[$task]}" 2>/dev/null || loud_fail "$task old Pi owner was not live during held assertion"
done
status=$("$WG_BIN" status)
[[ $(grep -c 'breaker-neutral' <<<"$status") -eq 4 ]] || loud_fail "wg status did not expose all breaker-neutral holds: $status"
for task in "${tasks[@]}"; do ! "$WG_BIN" ready | grep -q "$task" || loud_fail "$task held reopen leaked into ready queue"; done
[[ "$(sha256sum "${old_worktree[retry-owner]}/reopen-wip.txt" | awk '{print $1}')" == "$old_wip_hash" ]] || loud_fail "held reopen changed WIP"

# Three owners exit normally. The fourth reproduces the incident exactly:
# process+wrapper die without terminal bookkeeping, leaving watchdog Active.
for task in retry-owner requeue-owner reset-owner; do touch "$sync/$task.release-first"; done
kill -KILL "${old_pi_pid[stale-owner]}" "${old_wrapper_pid[stale-owner]}" 2>/dev/null || true
for _ in $(seq 1 200); do
  if ! kill -0 "${old_pi_pid[stale-owner]}" 2>/dev/null && ! kill -0 "${old_wrapper_pid[stale-owner]}" 2>/dev/null; then break; fi
  sleep 0.01
done
python3 - "${old_agent_dir[stale-owner]}/metadata.json" <<'PY'
import json,sys
m=json.load(open(sys.argv[1]))
s=json.load(open(m['attempt_runtime_dir']+'/pi/state.json'))['state']
assert s['classification']=='active' and not s['terminal'],s
PY
start_wg_daemon "$project" --max-agents 4 --no-chat-agent --interval 1
for task in "${tasks[@]}"; do
  for _ in $(seq 1 800); do [[ -s "$sync/$task.second.tsv" ]] && break; sleep 0.03; done
  [[ -s "$sync/$task.second.tsv" ]] || loud_fail "$task new generation did not launch after exact reap: $(tail -150 .wg/service/daemon.log 2>/dev/null || true)"
  ! kill -0 "${old_pi_pid[$task]}" 2>/dev/null || loud_fail "$task new generation overlapped exact old Pi process"
  ! kill -0 "${old_wrapper_pid[$task]}" 2>/dev/null || loud_fail "$task new generation overlapped old worktree-owning wrapper"
done

declare -A new_attempt new_session new_session_dir new_worktree new_pi_pid
for task in "${tasks[@]}"; do
  IFS=$'\t' read -r new_attempt[$task] new_session[$task] new_session_dir[$task] new_worktree[$task] new_pi_pid[$task] <"$sync/$task.second.tsv"
  [[ "${new_attempt[$task]}" == attempt-1-* || "${new_attempt[$task]}" == missing ]] || loud_fail "$task unexpected new attempt marker: ${new_attempt[$task]}"
  [[ "${new_worktree[$task]}" == "${old_worktree[$task]}" ]] || loud_fail "$task lost retained worktree"
  [[ -f "${new_worktree[$task]}/reopen-wip.txt" ]] || loud_fail "$task lost old WIP"
done
# Retry/requeue intentionally start fresh sessions. Reset preserves any task-level
# selector that already existed; this live fixture has only watchdog/session-dir
# evidence, which must remain retained even when the new spawn chooses a new id.
[[ "${new_session[retry-owner]}" != "${old_session[retry-owner]}" ]] || loud_fail "default retry unexpectedly reused old session"
[[ "${new_session[requeue-owner]}" != "${old_session[requeue-owner]}" ]] || loud_fail "requeue unexpectedly reused old session"
[[ "${new_session[stale-owner]}" != "${old_session[stale-owner]}" ]] || loud_fail "stale-owner retry unexpectedly reused old session"
[[ -d "${old_session_dir[reset-owner]}" ]] || loud_fail "reset lost old continuation session evidence"
[[ "$(sha256sum "${new_worktree[retry-owner]}/reopen-wip.txt" | awk '{print $1}')" == "$old_wip_hash" ]] || loud_fail "retry mutated old WIP"
[[ "$(find "${old_session_dir[retry-owner]}" -type f -name '*.jsonl' -print0 | sort -z | xargs -0 sha256sum | sha256sum | awk '{print $1}')" == "$old_session_hash" ]] || loud_fail "historical session evidence changed"

python3 - .wg/graph.jsonl .wg/service/registry.json <<'PY'
import json,sys
graph,registry=sys.argv[1:]
tasks={x['id']:x for x in map(json.loads,open(graph)) if x.get('kind')=='task'}
r=json.load(open(registry))
for tid,operation in [('retry-owner','retry'),('requeue-owner','requeue'),('reset-owner','reset'),('stale-owner','retry')]:
    t=tasks[tid]; l=t['lifecycle']; audit=l['audit']
    assert t['status']=='in-progress',t
    assert l['generation']==1 and l['current_attempt']['id'].startswith('attempt-1-'),l
    assert l.get('reopen_intent') is None,l
    assert sum(e['event_kind']=='reopen-requested' for e in audit)==1,audit
    assert sum(e['event_kind']=='reopen-owner-released' for e in audit)==1,audit
    assert sum(e['event_kind']=='attempt-reserved' and e['generation']==1 for e in audit)==1,audit
    assert t.get('spawn_failures',0)==0,t
    owners=[a for a in r['agents'].values() if a['task_id']==tid]
    assert len(owners)==2,owners
    assert sum(a['status'] in ('starting','working','idle') for a in owners)==1,owners
    assert sum(a['status']=='dead' for a in owners)==1,owners
PY

# A late old-owner terminal/progress call is stale evidence: its old source
# tuple remains in the immutable audit/runtime namespace and cannot change gen1.
old_runtime=$(python3 - "${old_agent_dir[retry-owner]}/metadata.json" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['attempt_runtime_dir'])
PY
)
[[ -d "$old_runtime" && -s "$old_runtime/pi/state.json" ]] || loud_fail "old runtime evidence was not retained"
before=$(sha256sum .wg/graph.jsonl | awk '{print $1}')
if env WG_TASK_ID=retry-owner WG_AGENT_ID="$(basename "${old_agent_dir[retry-owner]}")" WG_EXECUTOR_TYPE=pi \
  "$WG_BIN" pi-stream-bridge --agent-dir "${old_agent_dir[retry-owner]}" --exit-code 1 >/dev/null 2>&1; then
  : # evidence-only bridge; lifecycle hash still must not change
fi
after=$(sha256sum .wg/graph.jsonl | awk '{print $1}')
[[ "$before" == "$after" ]] || loud_fail "late old-owner evidence mutated reopened generation"

echo 'PASS: live Pi owners -> retry/requeue/reset holds plus stale-Active dead owner -> restart/exact reap -> one fenced generation each; breaker neutral with WIP/session evidence retained'
