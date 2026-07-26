#!/usr/bin/env bash
# Live credential-free authoritative-lifecycle acceptance flow.
# Drives the installed CLI plus the real long-lived daemon (including restart),
# never a library-only substitute.
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
wg init --no-agency -x shell >init.log 2>&1 || loud_fail "init failed: $(cat init.log)"
G="$project/.wg"
cleanup() { wg --dir "$G" service stop --force --kill-agents >/dev/null 2>&1 || true; }
trap cleanup EXIT

wg config --local --model pi:openrouter:openai/gpt-4o-mini --no-reload >/dev/null
wg config --auto-assign false --auto-evaluate false --flip-enabled false --no-reload >/dev/null
for id in terminal-done terminal-failed ordinary-open ordinary-running explicit-wait stale-owner; do
  wg add "$id" --id "$id" >/dev/null || loud_fail "add $id failed"
done
wg claim terminal-done --actor owner-done >/dev/null
env -u WG_AGENT_ID -u WG_TASK_ID wg done terminal-done >/dev/null || loud_fail "operator completion failed"
wg claim terminal-failed --actor owner-failed >/dev/null
env -u WG_AGENT_ID -u WG_TASK_ID wg fail terminal-failed --reason "intentional fixture" >/dev/null || loud_fail "fixture fail failed"
wg claim ordinary-running --actor owner-running >/dev/null

semantic_snapshot() {
  python3 - "$G/graph.jsonl" "$1" <<'PY'
import json,sys
path,tid=sys.argv[1:]
for line in open(path):
    row=json.loads(line)
    if row.get("kind")=="task" and row.get("id")==tid:
        lifecycle=row.get("lifecycle",{})
        out={
          "status":row.get("status","open"),
          "assigned":row.get("assigned"),
          "started_at":row.get("started_at"),
          "generation":lifecycle.get("generation",0),
          "revision":lifecycle.get("revision",0),
          "fence":lifecycle.get("fence",0),
          "attempt":lifecycle.get("current_attempt"),
          "spawn_failures":row.get("spawn_failures",0),
          "retry_count":row.get("retry_count",0),
          "wait_condition":row.get("wait_condition"),
        }
        print(json.dumps(out,sort_keys=True,separators=(",",":")))
        break
else: raise SystemExit("missing "+tid)
PY
}

ids=(terminal-done terminal-failed ordinary-open ordinary-running)
for id in "${ids[@]}"; do eval "before_${id//-/_}=\$(semantic_snapshot '$id')"; done

# Human terminal action: ordinary task messages through the actual wg CLI.
for id in "${ids[@]}"; do
  wg msg send "$id" "irrelevant message for $id" >/dev/null || loud_fail "message send failed for $id"
done

# Real daemon, repeated ticks, then a real stop/restart boundary.
wg --dir "$G" service start --max-agents 1 --no-chat-agent --interval 1 >start1.log 2>&1 \
  || loud_fail "daemon start failed: $(cat start1.log)"
sleep 3
for id in "${ids[@]}"; do
  var="before_${id//-/_}"; before="${!var}"; after=$(semantic_snapshot "$id")
  [[ "$after" == "$before" ]] || loud_fail "message/daemon mutated $id: before=$before after=$after log=$(tail -40 "$G/service/daemon.log")"
done
wg --dir "$G" service stop --force >/dev/null || loud_fail "first daemon stop failed"
wg --dir "$G" service start --max-agents 1 --no-chat-agent --interval 1 >start2.log 2>&1 \
  || loud_fail "daemon restart failed: $(cat start2.log)"
sleep 3
for id in "${ids[@]}"; do
  var="before_${id//-/_}"; before="${!var}"; after=$(semantic_snapshot "$id")
  [[ "$after" == "$before" ]] || loud_fail "restart/message replay mutated $id: before=$before after=$after"
done
echo "PASS (1/3): irrelevant messages cannot reopen/resume/keep alive Done, Failed, Open, or Running across live daemon ticks/restart"

# Explicit wait-on-message is the sole supported message lifecycle edge.
wg claim explicit-wait --actor waiter >/dev/null
WG_AGENT_ID=waiter wg wait explicit-wait --until message --checkpoint "parked" >/dev/null \
  || loud_fail "explicit wait failed"
[[ "$(semantic_snapshot explicit-wait)" == *'"status":"waiting"'* ]] || loud_fail "task did not enter Waiting"
wg msg send explicit-wait "explicit wake evidence" >/dev/null
for _ in $(seq 1 80); do
  snap=$(semantic_snapshot explicit-wait)
  [[ "$snap" == *'"status":"open"'* ]] && break
  sleep .1
done
[[ "$snap" == *'"status":"open"'* ]] || loud_fail "matching message did not satisfy explicit wait once: $snap"
wait_after="$snap"
sleep 2
[[ "$(semantic_snapshot explicit-wait)" == "$wait_after" ]] || loud_fail "same message satisfied explicit wait more than once"
echo "PASS (2/3): explicit persisted message wait wakes once; replay is inert"

# Stale worker fence: reset creates a greater generation, B claims it, and A's
# late completion must be rejected without changing B's attempt.
wg claim stale-owner --actor worker-A >/dev/null
old=$(semantic_snapshot stale-owner)
wg reset stale-owner --yes >/dev/null 2>&1 || loud_fail "reset failed"
wg claim stale-owner --actor worker-B >/dev/null
new_before=$(semantic_snapshot stale-owner)
if WG_TASK_ID=stale-owner WG_AGENT_ID=worker-A wg done stale-owner >late.out 2>late.err; then
  loud_fail "stale worker-A completion was accepted: $(cat late.out late.err)"
fi
new_after=$(semantic_snapshot stale-owner)
[[ "$new_after" == "$new_before" ]] || loud_fail "stale completion changed worker-B generation: before=$new_before after=$new_after old=$old err=$(cat late.err)"
[[ "$(cat late.err late.out)" == *"stale_attempt"* ]] || loud_fail "stale completion lacked stable rejection code: $(cat late.err late.out)"
wg msg send stale-owner "post-stale irrelevant" >/dev/null
sleep 2
[[ "$(semantic_snapshot stale-owner)" == "$new_before" ]] || loud_fail "post-stale message altered current owner"
echo "PASS (3/3): reset fenced worker A; late done rejected as stale_attempt; worker B and post-message state stayed exact"

events="$G/lifecycle/events.jsonl"
[[ -s "$events" ]] || loud_fail "authoritative lifecycle ledger missing"
python3 - "$events" <<'PY'
import json,sys
for n,line in enumerate(open(sys.argv[1]),1):
    frame=json.loads(line)
    assert frame.get("checksum")
    event=frame["event"]
    for key in ("event_id","idempotency_key","actor_kind","actor_id","reason_code","old_state","new_state","fence"):
        assert key in event,(n,key)
print("ledger audit metadata verified")
PY
cleanup
trap - EXIT
echo "PASS: authoritative lifecycle live human flow is fenced, idempotent, message-neutral, restart-safe, and audited"
