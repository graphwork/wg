#!/usr/bin/env bash
# Real installed-CLI + daemon proof that messages are durable data, never
# generic scheduler authority. The one exception is an exact, attempt-bound,
# one-shot Waiting(Message) subscription.
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
mkdir -p "$XDG_CONFIG_HOME"
cd "$project"
unset WG_TASK_ID WG_AGENT_ID WG_ATTEMPT_ID WG_ATTEMPT_GENERATION WG_ATTEMPT_FENCE || true
wg init --no-agency -x shell >init.log 2>&1 || loud_fail "init failed: $(cat init.log)"
G="$project/.wg"
live_pid=""
cleanup() {
  wg --dir "$G" service stop --force >/dev/null 2>&1 || true
  if [[ -n "$live_pid" ]]; then
    kill "$live_pid" >/dev/null 2>&1 || true
    wait "$live_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT

wg config --local --model pi:openrouter:openai/gpt-4o-mini --no-reload >/dev/null
wg config --auto-assign false --auto-evaluate false --flip-enabled false --no-reload >/dev/null
ids=(terminal-done terminal-failed terminal-abandoned dead-attempt live-running stale-epoch explicit-wait)
for id in "${ids[@]}"; do
  wg add "$id" --id "$id" >/dev/null || loud_fail "add $id failed"
done

wg claim terminal-done --actor owner-done >/dev/null
WG_AGENT_ID=owner-done WG_TASK_ID=terminal-done wg done terminal-done >/dev/null || loud_fail "done fixture failed"
wg claim terminal-failed --actor owner-failed >/dev/null
WG_AGENT_ID=owner-failed WG_TASK_ID=terminal-failed wg fail terminal-failed --reason fixture >/dev/null || loud_fail "failed fixture failed"
wg claim terminal-abandoned --actor owner-abandoned >/dev/null
wg abandon terminal-abandoned --reason fixture >/dev/null || loud_fail "abandoned fixture failed"
wg claim dead-attempt --actor owner-dead >/dev/null
wg claim live-running --actor owner-live >/dev/null

# A real live PID plus a matching registry entry makes this a genuine live
# attempt for delivery diagnostics. It occupies the sole daemon slot, keeping
# the spawn ledger deterministic and credential-free.
sleep 120 &
live_pid=$!
mkdir -p "$G/service" "$G/agents/owner-live"
now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
python3 - "$G/service/registry.json" "$live_pid" "$G/agents/owner-live/output.log" <<'PY'
import json,sys
path,pid,out=sys.argv[1:]
timestamp=__import__('datetime').datetime.now(__import__('datetime').timezone.utc).isoformat()
entry={
 "id":"owner-live","pid":int(pid),"task_id":"live-running","executor":"shell",
 "started_at":timestamp,"last_heartbeat":timestamp,
 "status":"working","output_file":out,"model":None,"completed_at":None,"worktree_path":None,
}
json.dump({"agents":{"owner-live":entry},"next_agent_id":1},open(path,"w"),sort_keys=True)
PY

# Bind history to epoch E, then explicitly create E+1. The old record must
# remain stale forever rather than floating to the new attempt.
wg claim stale-epoch --actor worker-old >/dev/null
wg msg send stale-epoch "old epoch history" --from user >/dev/null
wg reset stale-epoch --yes >/dev/null 2>&1 || loud_fail "stale fixture reset failed"
wg claim stale-epoch --actor worker-new >/dev/null

semantic_snapshot() {
  python3 - "$G/graph.jsonl" "$1" <<'PY'
import json,sys
path,tid=sys.argv[1:]
for line in open(path):
    row=json.loads(line)
    if row.get("kind")=="task" and row.get("id")==tid:
        lifecycle=row.get("lifecycle",{})
        out={
          "status":row.get("status","open"), "assigned":row.get("assigned"),
          "started_at":row.get("started_at"), "completed_at":row.get("completed_at"),
          "last_interaction_at":row.get("last_interaction_at"),
          "generation":lifecycle.get("generation",0), "revision":lifecycle.get("revision",0),
          "fence":lifecycle.get("fence",0), "attempt":lifecycle.get("current_attempt"),
          "spawn_failures":row.get("spawn_failures",0), "retry_count":row.get("retry_count",0),
          "wait_condition":row.get("wait_condition"), "message_wait":row.get("message_wait"),
          "working_dir":row.get("working_dir"), "agent":row.get("agent"),
        }
        print(json.dumps(out,sort_keys=True,separators=(",",":")))
        break
else: raise SystemExit("missing "+tid)
PY
}
registry_snapshot() {
  python3 - "$G/service/registry.json" <<'PY'
import json,sys
r=json.load(open(sys.argv[1]))
out={k:{x:v.get(x) for x in ("id","pid","task_id","executor","started_at","last_heartbeat","status","output_file","completed_at","worktree_path")} for k,v in r.get("agents",{}).items()}
print(json.dumps(out,sort_keys=True,separators=(",",":")))
PY
}
spawn_snapshot() {
  python3 - "$G/lifecycle/events.jsonl" "$G/agents" <<'PY'
import json,sys,os
events=sys.argv[1]
reserved=[]
if os.path.exists(events):
  for line in open(events):
    e=json.loads(line)["event"]
    if e.get("event_kind")=="attempt-reserved": reserved.append((e.get("task_id"),e.get("attempt_id"),e.get("fence")))
agent_dirs=sorted(x for x in os.listdir(sys.argv[2]) if os.path.isdir(os.path.join(sys.argv[2],x))) if os.path.isdir(sys.argv[2]) else []
print(json.dumps({"reserved":reserved,"agent_dirs":agent_dirs},sort_keys=True,separators=(",",":")))
PY
}
assert_unchanged() {
  local id=$1 before=$2 after
  after=$(semantic_snapshot "$id")
  [[ "$after" == "$before" ]] || loud_fail "message mutated $id: before=$before after=$after log=$(tail -60 "$G/service/daemon.log" 2>/dev/null || true)"
}

wg --dir "$G" service start --max-agents 2 --no-chat-agent --interval 1 >start1.log 2>&1 \
  || loud_fail "daemon start failed: $(cat start1.log)"
sleep 2
inert=(terminal-done terminal-failed terminal-abandoned dead-attempt live-running stale-epoch)
for id in "${inert[@]}"; do eval "before_${id//-/_}=\$(semantic_snapshot '$id')"; done
ready_before=$(wg --dir "$G" ready)
registry_before=$(registry_snapshot)
spawn_before=$(spawn_snapshot)

# Drive the actual operator ingress while the real daemon is running. Distinct
# messages exercise burst/idempotency pressure without giving unread counts any
# scheduler meaning.
for id in terminal-done terminal-failed terminal-abandoned dead-attempt stale-epoch; do
  for n in $(seq 1 12); do
    wg msg send "$id" "inert burst $n for $id" --from user >/dev/null || loud_fail "message send failed for $id"
  done
done
for n in $(seq 1 8); do wg msg send live-running "live context $n" --from user >/dev/null; done
wg msg read live-running --agent owner-live >/dev/null || loud_fail "live worker read failed"
sleep 4
for id in "${inert[@]}"; do var="before_${id//-/_}"; assert_unchanged "$id" "${!var}"; done
[[ "$(wg --dir "$G" ready)" == "$ready_before" ]] || loud_fail "inert messages changed ready membership"
[[ "$(registry_snapshot)" == "$registry_before" ]] || loud_fail "messages changed liveness/ownership registry"
[[ "$(spawn_snapshot)" == "$spawn_before" ]] || loud_fail "messages changed attempt/spawn ledger"
kill -0 "$live_pid" || loud_fail "live worker PID changed"
[[ ! -e "$G/messages/.respond-to-terminal-done.jsonl" ]] || loud_fail "response child inbox created"
[[ -z "$(find "$G" -name '.respond-to-*' -print -quit)" ]] || loud_fail "response child created"

# Exact machine-readable operator diagnostics: state + accepted recipient epoch
# are shown without treating unread/pending as execution state.
for pair in "terminal-done:terminal_task" "terminal-failed:terminal_task" "terminal-abandoned:terminal_task" "dead-attempt:dead_attempt" "live-running:delivered_live"; do
  id=${pair%%:*}; expected=${pair#*:}
  wg msg list "$id" --json >"$id.messages.json"
  python3 - "$id.messages.json" "$expected" <<'PY'
import json,sys
rows=json.load(open(sys.argv[1])); expected=sys.argv[2]
assert rows and all(r["disposition"]==expected for r in rows),(expected,rows)
for r in rows:
  assert "recipient_attempt_epoch" in r and "current_attempt_epoch" in r
  assert r["resume_requested"] is False and r["reason"]
PY
done
wg msg list stale-epoch --json >stale.messages.json
python3 - stale.messages.json <<'PY'
import json
rows=json.load(open('stale.messages.json'))
assert rows[0]['body']=='old epoch history' and rows[0]['disposition']=='stale_epoch',rows[0]
assert rows[0]['recipient_attempt_epoch'] != rows[0]['current_attempt_epoch'],rows[0]
PY

echo "PASS (1/3): bursts to Done, Failed, Abandoned, dead, stale, and live attempts preserved status/readiness/liveness/attempt/ownership/PID/spawn fingerprints; diagnostics are exact"

# Explicit current-attempt HumanInput is the positive/nonmatching control.
wg claim explicit-wait --actor waiter >/dev/null
WG_AGENT_ID=waiter WG_TASK_ID=explicit-wait wg wait explicit-wait --until human-input --checkpoint parked >/dev/null \
  || loud_fail "explicit wait failed"
wait_before=$(semantic_snapshot explicit-wait)
wg msg send explicit-wait "agent chatter is nonmatching" --from agent-other >/dev/null
sleep 2
[[ "$(semantic_snapshot explicit-wait)" == "$wait_before" ]] || loud_fail "nonmatching message resumed explicit wait"
wg msg list explicit-wait --json >wait.nonmatch.json
python3 - wait.nonmatch.json <<'PY'
import json
r=json.load(open('wait.nonmatch.json'))[-1]
assert r['disposition']=='waiting_nonmatch' and not r['resume_requested'],r
PY
for n in $(seq 1 20); do wg msg send explicit-wait "matching human $n" --from user >/dev/null; done
for _ in $(seq 1 80); do
  wait_after=$(semantic_snapshot explicit-wait)
  [[ "$wait_after" == *'"status":"open"'* ]] && break
  sleep .1
done
[[ "$wait_after" == *'"status":"open"'* ]] || loud_fail "matching message did not satisfy explicit wait: $wait_after"
sleep 2
[[ "$(semantic_snapshot explicit-wait)" == "$wait_after" ]] || loud_fail "duplicate messages resumed explicit wait repeatedly"
wg msg list explicit-wait --json >wait.consumed.json
python3 - wait.consumed.json "$G/lifecycle/events.jsonl" <<'PY'
import json,sys
rows=json.load(open(sys.argv[1]))
consumed=[r for r in rows if r['disposition']=='waiting_consumed']
assert len(consumed)==1,consumed
assert consumed[0]['resume_requested'] is True
assert all(not r['resume_requested'] for r in rows if r is not consumed[0])
events=[json.loads(x)['event'] for x in open(sys.argv[2])]
wakes=[e for e in events if e.get('task_id')=='explicit-wait' and e.get('event_kind')=='wait-satisfied']
assert len(wakes)==1,wakes
assert wakes[0]['reason_code']=='wait_condition_satisfied'
PY
echo "PASS (2/3): only the matching current-epoch subscription consumed once; nonmatch and 19 racing followers remained inert"

# Restart with all historical unread records. Capture the post-authorized-wake
# baseline separately; restart may not replay the consumed subscription or any
# inert historical message.
for id in "${inert[@]}" explicit-wait; do eval "restart_${id//-/_}=\$(semantic_snapshot '$id')"; done
restart_ready=$(wg --dir "$G" ready)
restart_registry=$(registry_snapshot)
restart_spawn=$(spawn_snapshot)
wg --dir "$G" service stop --force >/dev/null || loud_fail "first daemon stop failed"
wg --dir "$G" service start --max-agents 2 --no-chat-agent --interval 1 >start2.log 2>&1 \
  || loud_fail "daemon restart failed: $(cat start2.log)"
sleep 4
for id in "${inert[@]}" explicit-wait; do var="restart_${id//-/_}"; assert_unchanged "$id" "${!var}"; done
[[ "$(wg --dir "$G" ready)" == "$restart_ready" ]] || loud_fail "restart changed ready membership"
[[ "$(registry_snapshot)" == "$restart_registry" ]] || loud_fail "restart changed liveness/ownership"
[[ "$(spawn_snapshot)" == "$restart_spawn" ]] || loud_fail "restart replay spawned work"
log="$G/service/daemon.log"
! grep -E 'Resurrection:|\.respond-to-|reopened due to .*pending message' "$log" >/dev/null \
  || loud_fail "forbidden resurrection trace present: $(grep -E 'Resurrection:|\.respond-to-|reopened due to .*pending message' "$log")"

echo "PASS (3/3): daemon restart with historical unread messages produced zero reopen/readiness/liveness/ownership/attempt/spawn changes"
cleanup
trap - EXIT
echo "PASS: message delivery is durable, attempt-bound, diagnostic, restart-inert data; one armed subscription resumes exactly once"
