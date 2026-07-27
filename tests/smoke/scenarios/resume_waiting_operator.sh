#!/usr/bin/env bash
# Installed-CLI human flow: explicit operator resume is the only authority that
# releases a fail-closed legacy/unbound Waiting task, reusing its exact Pi
# session and dirty isolated worktree, and dispatches one resumed attempt.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 required"
scratch=$(make_scratch)
project="$scratch/project"
mkdir -p "$project" "$scratch/home"
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
export INITIAL_MARKER="$scratch/initial-attempt.log"
export RESUMED_MARKER="$scratch/resumed-attempts.log"
export PHASE_FILE="$scratch/initial-attempt-parked"
export FAKE_PID_FILE="$scratch/fake-pi.pid"
fakebin="$scratch/fakebin"
mkdir -p "$fakebin"
cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$$" >"$FAKE_PID_FILE"
session_id= session_dir=
argv="$*"
while (($#)); do
  case "$1" in
    --session-id) session_id=${2:-}; shift 2 ;;
    --session-dir) session_dir=${2:-}; shift 2 ;;
    *) shift ;;
  esac
done
if [[ ! -e "$PHASE_FILE" ]]; then
  : >"$PHASE_FILE"
  printf '%s|%s|%s|%s\n' "${WG_ATTEMPT_ID:-missing-attempt}" "$session_id" "$session_dir" "$PWD" >"$INITIAL_MARKER"
  printf '%s\n' 'uncommitted worktree WIP' > preserved-wip.txt
  PI_SESSION_ID="$session_id" wg wait "$WG_TASK_ID" --until message --checkpoint "exact prior checkpoint" >/dev/null
  exit 0
fi
printf '%s|%s|%s\n' "${WG_ATTEMPT_ID:-missing-attempt}" "$argv" "$PWD" >>"$RESUMED_MARKER"
sleep 20
SH
chmod +x "$fakebin/pi"
export PATH="$fakebin:$PATH"
cd "$project"
git init -q
git config user.email smoke@example.com
git config user.name smoke
echo base > README.md
git add README.md && git commit -qm base
wg init --no-agency >/dev/null || loud_fail "wg init failed"
G="$project/.wg"
cleanup_fake_pi() {
  if [[ -s "$FAKE_PID_FILE" ]]; then kill "$(cat "$FAKE_PID_FILE")" >/dev/null 2>&1 || true; fi
}
add_cleanup_hook cleanup_fake_pi
wg config --local --model pi:fake:fake-model --no-reload >/dev/null
wg config --auto-assign false --auto-evaluate false --flip-enabled false --no-reload >/dev/null
wg add "legacy waiting operator fixture" --id legacy-wait \
  --model pi:fake:fake-model --exec-mode full >/dev/null
wg publish legacy-wait --only >/dev/null
start_wg_daemon "$project" --max-agents 1 --no-chat-agent --interval 1 \
  || loud_fail "service start failed"

# The first real daemon attempt creates the isolated worktree and Pi session,
# writes dirty WIP there, and parks itself on a message wait.
for _ in $(seq 1 120); do
  if [[ -s "$INITIAL_MARKER" ]] && python3 - "$G/graph.jsonl" <<'PY' >/dev/null 2>&1
import json,sys
task=next(r for r in map(json.loads,open(sys.argv[1])) if r.get('kind')=='task' and r.get('id')=='legacy-wait')
assert task['status']=='waiting'
PY
  then break; fi
  sleep .1
done
[[ -s "$INITIAL_MARKER" ]] || loud_fail "initial worker did not park"
IFS='|' read -r initial_attempt exact_session_id prior_session prior_worktree <"$INITIAL_MARKER"
[[ -n "$exact_session_id" && -f "$prior_session/wg_${exact_session_id}.jsonl" ]] \
  || loud_fail "initial worker did not create an exact Pi session attestation"
[[ "$(cat "$prior_worktree/preserved-wip.txt")" == 'uncommitted worktree WIP' ]] \
  || loud_fail "initial worker did not leave WIP in its isolated worktree"

# Let the parked wrapper exit completely so its verified worktree is eligible
# for the resumed attempt's retry-in-place ownership check.
prior_agent_dir=${prior_session%/pi-session}
prior_wrapper_pid=$(python3 - "$prior_agent_dir/metadata.json" <<'PY'
import json,sys
print(json.load(open(sys.argv[1]))['pid'])
PY
)
for _ in $(seq 1 80); do
  kill -0 "$prior_wrapper_pid" >/dev/null 2>&1 || break
  sleep .1
done
kill -0 "$prior_wrapper_pid" >/dev/null 2>&1 \
  && loud_fail "parked wrapper did not exit before operator resume"

# Convert only the fixture metadata to the historical pre-subscription shape.
# Recovery below is entirely through the public CLI, with no graph surgery.
python3 - "$G/graph.jsonl" "$exact_session_id" <<'PY'
import json,sys
p,sid=sys.argv[1:]; rows=[]
for line in open(p):
    row=json.loads(line)
    if row.get("kind")=="task" and row.get("id")=="legacy-wait":
        row.pop("message_wait",None)
        row["session_id"]=sid
    rows.append(row)
with open(p,"w") as f:
    for row in rows: f.write(json.dumps(row,separators=(",",":"))+"\n")
PY
before=$(python3 - "$G/graph.jsonl" <<'PY'
import json,sys
for line in open(sys.argv[1]):
 r=json.loads(line)
 if r.get('kind')=='task' and r.get('id')=='legacy-wait':
  l=r.get('lifecycle',{}); print(json.dumps({'generation':l.get('generation',0),'attempt_sequence':l.get('attempt_sequence',0),'retry_count':r.get('retry_count',0),'assigned':r.get('assigned')})); break
PY
)

wg msg send legacy-wait "ordinary legacy message is inert" --from user >/dev/null
sleep 2
python3 - "$G/graph.jsonl" "$G/messages/legacy-wait.jsonl" <<'PY'
import json,sys
task=next(r for r in map(json.loads,open(sys.argv[1])) if r.get('kind')=='task' and r.get('id')=='legacy-wait')
assert task['status']=='waiting',task
msg=json.loads(open(sys.argv[2]).readline())
assert msg['accepted_disposition']=='legacy_unbound',msg
PY
[[ ! -e "$RESUMED_MARKER" ]] || loud_fail "ordinary legacy message launched a worker"

# Actual operator terminal flow. The second invocation may race with dispatch;
# both Open and InProgress are required to be idempotent successes.
wg resume legacy-wait --only >resume-1.log 2>&1 \
  || loud_fail "explicit waiting resume failed: $(cat resume-1.log)"
wg resume legacy-wait --only >resume-2.log 2>&1 \
  || loud_fail "repeated waiting resume was not idempotent: $(cat resume-2.log)"
for _ in $(seq 1 80); do [[ -s "$RESUMED_MARKER" ]] && break; sleep .1; done
[[ -s "$RESUMED_MARKER" ]] || loud_fail "resume did not kick dispatch"
sleep 2
[[ "$(wc -l < "$RESUMED_MARKER" | tr -d ' ')" == 1 ]] \
  || loud_fail "resume launched more than one process: $(cat "$RESUMED_MARKER")"
grep -q -- "--session-id $exact_session_id" "$RESUMED_MARKER" \
  || loud_fail "resumed Pi launch did not use the exact prior session: $(cat "$RESUMED_MARKER")"
grep -q -- "--session-dir $prior_session" "$RESUMED_MARKER" \
  || loud_fail "resumed Pi launch did not reuse the prior session directory: $(cat "$RESUMED_MARKER")"
resumed_worktree=$(cut -d'|' -f3 "$RESUMED_MARKER")
[[ "$resumed_worktree" == "$prior_worktree" ]] \
  || loud_fail "resume did not reuse prior worktree: prior=$prior_worktree resumed=$resumed_worktree"
[[ "$(cat "$prior_worktree/preserved-wip.txt")" == 'uncommitted worktree WIP' ]] \
  || loud_fail "resume changed preserved worktree WIP"

python3 - "$G/graph.jsonl" "$G/lifecycle/events.jsonl" "$before" <<'PY'
import json,sys
task=next(r for r in map(json.loads,open(sys.argv[1])) if r.get('kind')=='task' and r.get('id')=='legacy-wait')
events=[json.loads(x)['event'] for x in open(sys.argv[2])]
before=json.loads(sys.argv[3]); lifecycle=task.get('lifecycle',{})
wakes=[e for e in events if e.get('task_id')=='legacy-wait' and e.get('event_kind')=='wait-satisfied']
assert len(wakes)==1,wakes
wake=wakes[0]
assert wake['old_state']=='waiting' and wake['new_state']=='open',wake
assert wake['actor_kind']=='operator' and wake['reason_code']=='operator_resume',wake
assert len(wake.get('evidence_refs',[]))==1 and wake['evidence_refs'][0].startswith('operator-receipt:'),wake
reserved=[e for e in events if e.get('task_id')=='legacy-wait' and e.get('event_kind')=='attempt-reserved']
assert len(reserved)==2,reserved # initial worker + exactly one resumed attempt
assert not [e for e in events if e.get('task_id')=='legacy-wait' and e.get('event_kind') in ('attempt-failed','attempt-lost')],events
assert lifecycle.get('generation',0)==before['generation'],(before,lifecycle)
assert lifecycle.get('attempt_sequence',0)==before['attempt_sequence']+1,(before,lifecycle)
assert task.get('retry_count',0)==before['retry_count'],(before,task)
assert task.get('checkpoint')=='exact prior checkpoint',task
assert task.get('session_id'),task
assert task.get('wait_condition') is None,task
assert task.get('message_wait') is None,task
assert task.get('assigned') and task.get('assigned')!=before['assigned'],(before,task)
assert task['status']=='in-progress',task
PY

echo "PASS: legacy messages stayed inert; operator resume appended one fenced receipt, reused exact Pi session + dirty worktree, and dispatched one same-generation attempt"
