#!/usr/bin/env bash
# Integrated credential-free proof for the service-owned convergence cutover.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
unset WG_AGENT_ID WG_DIR WG_PROJECT_ROOT WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH WG_TASK_ID

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME" "$scratch/bin"
cd "$scratch"

wg init --no-agency >init.log 2>&1 || loud_fail "wg init failed: $(cat init.log)"
wg_dir="$scratch/.wg"
wg --dir "$wg_dir" config --local -m pi:openrouter:example/convergence-smoke --no-reload \
  >config.log 2>&1 || loud_fail "route config failed: $(cat config.log)"
# Fast bounded policy; safety interval stays slow so persisted deadlines are the
# observable wake authority.
for kv in \
  dispatcher.poll_interval=60 \
  dispatcher.settling_delay_ms=20 \
  agency.auto_assign=false \
  dispatcher.convergence.base_seconds=1 \
  dispatcher.convergence.cap_seconds=2 \
  dispatcher.convergence.route_probe_base_seconds=1 \
  dispatcher.convergence.route_probe_cap_seconds=2 \
  dispatcher.convergence.action_lease_seconds=2 \
  dispatcher.convergence.jitter_divisor=1000000
do
  key=${kv%%=*}; value=${kv#*=}
  wg --dir "$wg_dir" config set "$key" "$value" >>config.log 2>&1 \
    || loud_fail "config set $kv failed: $(cat config.log)"
done

wg --dir "$wg_dir" add "paused convergence goal" --id convergence-goal \
  -d "A visible goal used only to inspect deterministic wake state." >/dev/null
wg --dir "$wg_dir" publish convergence-goal --only >/dev/null
wg --dir "$wg_dir" pause convergence-goal >/dev/null

state="$wg_dir/service/convergence-state.json"
start_wg_daemon "$scratch" --no-chat-agent --force --max-agents 1
for _ in $(seq 1 100); do
  [[ -f "$state" ]] && python3 - "$state" <<'PY' >/dev/null 2>&1 && break
import json, sys
s=json.load(open(sys.argv[1]))
r=s["goals"]["convergence-goal#0"]
assert r["stage"] == "await-dispatch"
assert r["backoff"]["jitter_seed"].startswith("b3:")
PY
  sleep 0.1
done
[[ -f "$state" ]] || loud_fail "convergence state was not created"
wg --dir "$wg_dir" service stop --force --kill-agents >/dev/null 2>&1 || true

# Seed a long future deadline and non-zero exponent, restart, and prove startup
# derivation does not redraw/reset the scheduling tuple.
python3 - "$state" "$scratch/before.json" <<'PY'
import datetime, json, sys
path,out=sys.argv[1:]
s=json.load(open(path)); r=s["goals"]["convergence-goal#0"]
r["backoff"]["failures_without_progress"]=6
r["next_wake_at"]=(datetime.datetime.now(datetime.timezone.utc)+datetime.timedelta(minutes=5)).isoformat()
r["pending_action"]=None
json.dump({"next_wake_at":r["next_wake_at"],"backoff":r["backoff"]},open(out,"w"),sort_keys=True)
json.dump(s,open(path,"w"),indent=2,sort_keys=True)
PY
start_wg_daemon "$scratch" --no-chat-agent --force --max-agents 1
sleep 0.5
python3 - "$state" "$scratch/before.json" <<'PY' 
import json, sys
s=json.load(open(sys.argv[1])); before=json.load(open(sys.argv[2]))
r=s["goals"]["convergence-goal#0"]
assert r["next_wake_at"] == before["next_wake_at"], (r,before)
assert r["backoff"] == before["backoff"], (r,before)
PY
wg --dir "$wg_dir" service stop --force --kill-agents >/dev/null 2>&1 || true
echo "PASS (1/3): daemon restart preserved deadline, exponent, and deterministic jitter seed"

# Make the same record overdue. The real service loop must wake from the
# persisted deadline (not the 60s safety interval), advance one unchanged pass,
# retain the task as live, and cap rather than exhaust.
python3 - "$state" <<'PY'
import datetime, json, sys
p=sys.argv[1]; s=json.load(open(p)); r=s["goals"]["convergence-goal#0"]
r["next_wake_at"]=(datetime.datetime.now(datetime.timezone.utc)-datetime.timedelta(seconds=1)).isoformat()
r["pending_action"]=None
json.dump(s,open(p,"w"),indent=2,sort_keys=True)
PY
start_wg_daemon "$scratch" --no-chat-agent --force --max-agents 1
advanced=false
for _ in $(seq 1 100); do
  if python3 - "$state" <<'PY' >/dev/null 2>&1
import datetime, json, sys
r=json.load(open(sys.argv[1]))["goals"]["convergence-goal#0"]
assert r["backoff"]["failures_without_progress"] >= 7
next_at=datetime.datetime.fromisoformat(r["next_wake_at"].replace("Z","+00:00"))
assert next_at > datetime.datetime.now(datetime.timezone.utc)
PY
  then advanced=true; break; fi
  sleep 0.1
done
$advanced || loud_fail "persisted overdue wake did not advance before the 60s safety interval"
status=$(wg --dir "$wg_dir" show convergence-goal)
grep -q '^Status: open' <<<"$status" || loud_fail "long transient falloff terminalized the goal: $status"
wg --dir "$wg_dir" service stop --force --kill-agents >/dev/null 2>&1 || true
echo "PASS (2/3): overdue unchanged goal woke exponentially and stayed live at the cap"

# Fake Pi keeps the one admitted probe alive. No network or credential is used.
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
for _ in $(seq 1 150); do
  if python3 - "$state" "$wg_dir/agents/registry.json" <<'PY' >/dev/null 2>&1
import json, os, sys
s=json.load(open(sys.argv[1])); b=s["route_breakers"]["pi|openrouter|self-authenticated"]
assert b["state"] == "probing"
assert b["probe_lease"]["task_id"] in ("route-a","route-b")
# The exact-route lease is singular by schema, and no global controller exists.
assert len([x for x in s["route_breakers"].values() if x.get("probe_lease")]) == 1
reg=json.load(open(sys.argv[2])) if os.path.exists(sys.argv[2]) else {"agents":{}}
live=[a for a in reg.get("agents",{}).values() if a.get("task_id") in ("route-a","route-b") and a.get("status") in ("working","alive","running")]
assert len(live) <= 1, live
PY
  then probe=true; break; fi
  sleep 0.2
done
$probe || loud_fail "one exact-route probe lease was not observed: $(cat "$state" 2>/dev/null || true)"
python3 - "$wg_dir/graph.jsonl" <<'PY'
import json, sys
rows=[json.loads(line) for line in open(sys.argv[1]) if line.strip()]
ids=[r.get("id","") for r in rows]
for prefix in (".daemon-",".supervisor-",".probe-",".merge-",".cleanup-"):
    assert not any(i.startswith(prefix) for i in ids), (prefix,ids)
PY
wg --dir "$wg_dir" service stop --force --kill-agents >/dev/null 2>&1 || true
echo "PASS (3/3): one exact-route probe, no fallback/global pause, no controller graph task"

echo "PASS: post-lifecycle convergence daemon/restart/provider-breaker integration"
