#!/usr/bin/env bash
# Scenario: heartbeat_external_interop_no_resurrect
#
# Reproduces the interop contract between an external heartbeat
# (`wg heartbeat agent-N`) and the WG service's own agent-reaper
# (`triage::cleanup_dead_agents` / `coordinator_tick`).
#
# CONTRACT under test (src/commands/heartbeat.rs::run_agent +
# src/commands/service/triage.rs::detect_dead_reason +
# src/commands/service/coordinator.rs coordinator_tick):
#
#  1. LIVE agent + external heartbeat ⇒ the agent SURVIVES the next
#     coordinator tick (fresh heartbeat prevents the HeartbeatTimeout
#     reap path). External heartbeat is a legitimate keep-alive.
#  2. DEAD agent (PID gone) + external heartbeat ⇒ the agent is STILL
#     reaped on the next coordinator tick. The fresh heartbeat does
#     NOT resurrect a dead process — `detect_dead_reason` checks
#     `is_process_alive(pid)` BEFORE heartbeat, so a heartbeat for a
#     gone process is ineffective. This is the "external heartbeat
#     does not fight the service" guarantee.
#
#  This pins the interop so a future change that reordered the
#  heartbeat-before-process check (and thus let an external heartbeat
#  resurrect a dead agent's slot) would fail loudly.
#
# No LLM credentials. We use `wg service tick` (single coordinator tick,
# no daemon loop) and seed the registry directly with a real `sleep` PID.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON3" "python3 required to seed registry.json"
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

wg_dir="$scratch/.wg"
registry="$wg_dir/service/registry.json"
mkdir -p "$wg_dir/service"

# A long-running sleep process stands in for a "live agent". Its PID is
# what we register. is_process_alive(pid) returns true for it.
sleep 300 &
live_pid=$!
add_cleanup_hook() { kill "$live_pid" 2>/dev/null || true; }

# ── Seed the registry with one agent pointing at the live PID ────────────
# started_at must be RECENT: detect_dead_reason's PID-reuse guard
# (verify_process_identity) requires the registered start time to be within
# ~120s of the actual process start, or the agent is reaped as PidReused.
# We also set last_heartbeat stale (>heartbeat_timeout=5min) so the external
# `wg heartbeat` call is the thing that refreshes it — proving the call path.
python3 - "$registry" "$live_pid" <<'PY'
import json, sys, os, datetime
path, pid = sys.argv[1], int(sys.argv[2])
os.makedirs(os.path.dirname(path), exist_ok=True)
now = datetime.datetime.now(datetime.timezone.utc)
# started_at = 40s ago: past the 30s reaper grace, but within the 120s
# PID-identity slack of the actual `sleep` process start.
started = now - datetime.timedelta(seconds=40)
# last_heartbeat = 1h ago: stale beyond the 5min heartbeat_timeout, so a tick
# WITHOUT an external heartbeat would reap via HeartbeatTimeout.
heartbeat = now - datetime.timedelta(hours=1)
reg = {
    "next_agent_id": 2,
    "agents": {
        "agent-1": {
            "id": "agent-1",
            "pid": pid,
            "task_id": "noop-task",
            "executor": "shell",
            "started_at": started.strftime("%Y-%m-%dT%H:%M:%SZ"),
            "last_heartbeat": heartbeat.strftime("%Y-%m-%dT%H:%M:%SZ"),
            "status": "working",
            "output_file": "/dev/null",
            "model": None,
            "completed_at": None,
            "worktree_path": None,
        }
    },
}
with open(path, "w") as f:
    json.dump(reg, f, indent=2)
PY

read_field() {
    local field="$1"
    python3 -c "import json,sys; print(json.load(open('$registry'))['agents']['agent-1'].get('$field'))"
}
read_status() { read_field "status"; }
read_heartbeat() { read_field "last_heartbeat"; }

# Sanity: agent started with a stale heartbeat but a LIVE process.
if [[ "$(read_status)" != "working" ]]; then
    loud_fail "seed registry: agent-1 status not working (got: $(read_status))"
fi

# ── Test 1: external heartbeat on LIVE agent keeps it alive across a tick ─
hb_out=$(wg heartbeat agent-1 2>&1) || loud_fail "wg heartbeat agent-1 failed: $hb_out"
if ! echo "$hb_out" | grep -q "heartbeat recorded"; then
    loud_fail "wg heartbeat did not confirm recording: $hb_out"
fi
fresh_hb=$(read_heartbeat)
if [[ "$fresh_hb" == "2026-07-05T00:00:00Z" ]]; then
    loud_fail "external heartbeat did not update last_heartbeat (still stale)"
fi

# Run a single coordinator tick. With a live process + fresh heartbeat,
# detect_dead_reason must return None and the agent stays Working.
tick_out=$(wg --dir "$wg_dir" service tick --max-agents 0 2>&1) || true
if [[ "$(read_status)" != "working" ]]; then
    loud_fail "LIVE agent was reaped after an external heartbeat — heartbeat keep-alive is broken. tick_out:\n$tick_out\nstatus=$(read_status)"
fi
echo "PASS (1/2): external heartbeat keeps a LIVE agent alive across a coordinator tick"

# ── Test 2: external heartbeat on DEAD agent does NOT resurrect it ───────
# Kill the live process so is_process_alive(pid) returns false. Then send
# an external heartbeat (fresh timestamp). The next coordinator tick must
# STILL reap the agent — ProcessExited is checked before heartbeat.
kill "$live_pid" 2>/dev/null || true
# Reap the zombie so is_process_alive reports false cleanly.
wait "$live_pid" 2>/dev/null || true
sleep 1
if kill -0 "$live_pid" 2>/dev/null; then
    # Process didn't die — give it SIGKILL.
    kill -9 "$live_pid" 2>/dev/null || true
    sleep 1
fi

hb_out=$(wg heartbeat agent-1 2>&1) || loud_fail "wg heartbeat agent-1 (dead) failed: $hb_out"
# Heartbeat itself succeeds (it just writes the registry) — that's fine.
dead_hb=$(read_heartbeat)
if [[ "$dead_hb" == "$fresh_hb" ]]; then
    loud_fail "external heartbeat did not advance last_heartbeat on dead agent (expected newer than $fresh_hb, got $dead_hb)"
fi

# Coordinator tick: must reap the agent despite the fresh heartbeat.
tick_out=$(wg --dir "$wg_dir" service tick --max-agents 0 2>&1) || true
final_status=$(read_status)
if [[ "$final_status" != "dead" ]]; then
    loud_fail "DEAD agent was NOT reaped after a coordinator tick despite its process being gone — an external heartbeat resurrected a dead slot. tick_out:\n$tick_out\nstatus=$final_status\npid=$live_pid (alive? $(kill -0 "$live_pid" 2>/dev/null && echo yes || echo no))"
fi
echo "PASS (2/2): external heartbeat does NOT resurrect a DEAD agent (process-liveness reaper wins)"

echo "PASS: external heartbeat interops correctly with the WG service reaper"
exit 0
