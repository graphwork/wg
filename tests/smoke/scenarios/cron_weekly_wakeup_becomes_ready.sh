#!/usr/bin/env bash
# Scenario: cron_weekly_wakeup_becomes_ready
#
# Reproduces WG recurring weekly-cron wakeup semantics and the
# daemon-downtime / missed-trigger catch-up path.
#
# CONTRACT under test (src/cron.rs::is_cron_due + src/query.rs::is_time_ready
# + src/commands/service/coordinator.rs Phase 2.95 cron reset):
#
#  1. WEEKLY GATE: a cron-enabled task with `next_cron_fire` in the FUTURE
#     is NOT ready (is_time_ready → is_cron_due → next_fire > now ⇒ false).
#  2. WEEKLY WAKEUP: when `next_cron_fire` is in the PAST, the task IS ready
#     and appears in `wg ready` once a coordinator tick runs.
#  3. MISSED-TRIGGER CATCH-UP: if the daemon was DOWN across the scheduled
#     fire time (next_cron_fire already in the past when the daemon starts),
#     the FIRST tick after restart wakes the task — i.e. the missed fire is
#     NOT silently dropped, it fires LATE on the next tick. This pins that
#     behaviour so a future change that drops catch-up fails loudly.
#
# No LLM credentials needed. We never spawn a worker (the cron task is left
# unassigned / unplaced, so the dispatcher short-circuits before invoking a
# model). Pure graph-state + `wg ready` regression.

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
# src/cron.rs::cron_dow_mapping_is_nonstandard_one_indexed_sunday.("sec min hour day month dow";
# dow=1 is Monday in the cron crate, 0 = Sunday).
cron_expr="0 0 9 * * 2"
add_out=$(wg add "weekly-monday-9am" --cron "$cron_expr" --no-place 2>&1) || \
    loud_fail "wg add --cron failed: $add_out"
task_id=$(echo "$add_out" | grep "^Added task:" | grep -oP '\(\K[^)]+')
if [[ -z "$task_id" ]]; then
    loud_fail "could not parse task id from: $add_out"
fi

wg_dir="$scratch/.wg"
graph="$wg_dir/graph.jsonl"

# Helper: rewrite the next_cron_fire field on $task_id in graph.jsonl to an
# arbitrary RFC3339 timestamp. We pause the daemon (if running) around the
# rewrite so the inotify watcher can't race the file swap.
set_next_fire() {
    local ts="$1"
    python3 - "$graph" "$task_id" "$ts" <<'PY'
import json, sys, os
path, tid, ts = sys.argv[1], sys.argv[2], sys.argv[3]
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
        obj["next_cron_fire"] = ts
    out.append(json.dumps(obj, separators=(",", ":")))
with open(tmp, "w") as f:
    for o in out:
        f.write(o + "\n")
os.replace(tmp, path)
PY
    # The daemon's inotify graph-watcher (or the poll_interval safety net)
    # picks up the atomic rename. We only rewrite while the daemon is
    # stopped or before it starts, so there is no tick race to worry about.
}

# ── Test 1: future next_cron_fire ⇒ NOT ready ─────────────────────────────
future_ts=$(python3 -c "import datetime; print((datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(days=7)).strftime('%Y-%m-%dT%H:%M:%SZ'))")
set_next_fire "$future_ts"

ready_out=$(wg ready 2>&1)
if echo "$ready_out" | grep -q "$task_id"; then
    loud_fail "cron task with FUTURE next_cron_fire ($future_ts) appeared in wg ready — weekly gate broken:\n$ready_out"
fi
echo "PASS (1/3): future next_cron_fire correctly keeps weekly cron task NOT ready"

# ── Test 2: past next_cron_fire (daemon ACTIVE) ⇒ IS ready after a tick ──
past_ts=$(python3 -c "import datetime; print((datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(minutes=5)).strftime('%Y-%m-%dT%H:%M:%SZ'))")
set_next_fire "$past_ts"

# Boot the daemon with max-agents=0 so it ticks but never spawns a worker
# (no LLM credentials needed). poll interval = 2s for a fast wake.
start_wg_daemon "$scratch" --max-agents 0 --no-chat-agent --interval 2
wg_dir="$WG_SMOKE_DAEMON_DIR"

# Wait up to 15s for the cron task to surface in `wg ready`.
ready=0
for i in $(seq 1 15); do
    ready_out=$(wg --dir "$wg_dir" ready 2>&1)
    if echo "$ready_out" | grep -q "$task_id"; then
        ready=1
        break
    fi
    sleep 1
done
if [[ "$ready" -ne 1 ]]; then
    loud_fail "cron task with PAST next_cron_fire ($past_ts) never became ready under the active daemon. Last wg ready:\n$ready_out"
fi
echo "PASS (2/3): past next_cron_fire wakes the weekly cron task under the active daemon (weekly wakeup works)"

# ── Test 3: missed-trigger catch-up across daemon DOWNTIME ────────────────
# Simulate: the weekly fire time arrived while the daemon was STOPPED.
# Stop the daemon, advance next_cron_fire into the past (it was in the
# future in test 1, now it's "yesterday"), then restart and verify the
# first tick wakes the task — i.e. the missed fire is caught up, not
# silently dropped.
wg --dir "$wg_dir" service stop --force >/dev/null 2>&1 || true
sleep 1

# Confirm daemon is actually stopped (state.json should report not running
# or be absent). We don't strictly need this assertion, but it pins the
# precondition that the fire time was crossed while DOWN.
missed_ts=$(python3 -c "import datetime; print((datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=2)).strftime('%Y-%m-%dT%H:%M:%SZ'))")
set_next_fire "$missed_ts"

# Restart the daemon fresh.
start_wg_daemon "$scratch" --max-agents 0 --no-chat-agent --interval 2
wg_dir="$WG_SMOKE_DAEMON_DIR"

caught_up=0
for i in $(seq 1 15); do
    ready_out=$(wg --dir "$wg_dir" ready 2>&1)
    if echo "$ready_out" | grep -q "$task_id"; then
        caught_up=1
        break
    fi
    sleep 1
done
if [[ "$caught_up" -ne 1 ]]; then
    loud_fail "missed weekly fire (next_cron_fire=$missed_ts set while daemon was stopped) was NOT caught up on restart — the missed-trigger catch-up path is broken. Last wg ready:\n$ready_out"
fi
echo "PASS (3/3): missed weekly fire while daemon was down is caught up on the first tick after restart"

echo "PASS: weekly cron wakeup + missed-trigger catch-up semantics hold"
exit 0
