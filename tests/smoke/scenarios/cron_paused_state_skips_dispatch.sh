#!/usr/bin/env bash
# Scenario: cron_paused_state_skips_dispatch
#
# Reproduces the interaction between recurring cron triggers and the
# paused / waiting task states. A cron task that is DUE (next_cron_fire in
# the past) must still respect task-state gates:
#
#  CONTRACT (src/query.rs::ready_tasks_with_peers_cycle_aware):
#    - status must be Open|Incomplete        (a Waiting cron task is NOT ready)
#    - task.paused must be false             (a paused cron task is NOT ready)
#    - is_time_ready (cron gate) must pass   (next_cron_fire <= now)
#
#  This pins that a recurring trigger does NOT bypass the paused/waiting
#  gates — i.e. an operator who pauses a weekly cron task (e.g. to stage a
#  maintenance window) will NOT have it auto-dispatched when the fire time
#  arrives, and a Waiting cron task (e.g. one parked on a subtask) is also
#  held back even when the schedule says "fire now".
#
# No LLM credentials. No daemon worker spawn (max-agents=0). Pure
# graph-state + `wg ready` regression.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 required to rewrite next_cron_fire in graph.jsonl"
fi

scratch=$(make_scratch)
cd "$scratch"

# Isolate HOME + XDG so the host global ~/.wg/config.toml cannot
# leak a stale executor/model or a duplicate poll_interval into the merged
# config (the host install has its own [dispatcher] section).
fake_home="$scratch/home"
mkdir -p "$fake_home/.config"
export HOME="$fake_home"
export XDG_CONFIG_HOME="$fake_home/.config"

if ! wg init -x shell >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -5 init.log)"
fi

# A weekly cron: every Monday 09:00 UTC. dow=2 is Monday in the
# `cron` crate (0.12.x), which uses a NON-STANDARD 1=Sunday mapping — see
# src/cron.rs::cron_dow_mapping_is_nonstandard_one_indexed_sunday.
cron_expr="0 0 9 * * 2"
add_out=$(wg add "weekly-paused" --cron "$cron_expr" --no-place 2>&1) || \
    loud_fail "wg add --cron failed: $add_out"
task_id=$(echo "$add_out" | grep "^Added task:" | grep -oP '\(\K[^)]+')
if [[ -z "$task_id" ]]; then
    loud_fail "could not parse task id from: $add_out"
fi

wg_dir="$scratch/.wg"
graph="$wg_dir/graph.jsonl"
past_ts=$(python3 -c "import datetime; print((datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(minutes=5)).strftime('%Y-%m-%dT%H:%M:%SZ'))")

set_field() {
    local field="$1" val="$2"
    python3 - "$graph" "$task_id" "$field" "$val" <<'PY'
import json, sys, os
path, tid, field, val = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
tmp = path + ".tmp.%d" % os.getpid()
with open(path) as f:
    lines = f.readlines()
out = []
for line in lines:
    line = line.rstrip("\n")
    if not line.strip():
        continue
    obj = json.loads(line)
    if obj.get("kind") == "task" and obj.get("id") == tid:
        if val == "__DELETE__":
            obj.pop(field, None)
        elif val == "true":
            obj[field] = True
        elif val == "false":
            obj[field] = False
        else:
            obj[field] = val
    out.append(json.dumps(obj, separators=(",", ":")))
with open(tmp, "w") as f:
    for o in out:
        f.write(o + "\n")
os.replace(tmp, path)
PY
}

# ── Test 1: due + NOT paused ⇒ ready (baseline) ───────────────────────────
set_field "next_cron_fire" "$past_ts"
set_field "paused" "false"
ready_out=$(wg ready 2>&1)
if ! echo "$ready_out" | grep -q "$task_id"; then
    loud_fail "baseline: due + unpaused cron task should be ready, but wg ready didn't list it:\n$ready_out"
fi
echo "PASS (1/3): due + unpaused cron task is ready (baseline)"

# ── Test 2: due + paused ⇒ NOT ready ──────────────────────────────────────
set_field "paused" "true"
ready_out=$(wg ready 2>&1)
if echo "$ready_out" | grep -q "$task_id"; then
    loud_fail "paused cron task appeared in wg ready even though next_cron_fire is due — paused gate broken:\n$ready_out"
fi
echo "PASS (2/3): due + paused cron task is correctly held back (paused gate holds)"

# ── Test 3: due + Waiting status ⇒ NOT ready ─────────────────────────────
# A Waiting task (parked on a subtask) must NOT be woken by the cron trigger
# alone — the status gate in ready_tasks_with_peers_cycle_aware filters it
# out before is_time_ready is ever consulted.
set_field "paused" "false"
set_field "status" "waiting"
ready_out=$(wg ready 2>&1)
if echo "$ready_out" | grep -q "$task_id"; then
    loud_fail "Waiting-status cron task appeared in wg ready — status gate broken:\n$ready_out"
fi
echo "PASS (3/3): due + Waiting cron task is correctly held back (status gate holds)"

echo "PASS: paused / waiting state correctly gates recurring cron triggers"
exit 0
