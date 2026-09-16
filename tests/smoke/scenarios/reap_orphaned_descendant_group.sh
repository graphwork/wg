#!/usr/bin/env bash
# Regression for reap-orphan-descendant-pgroups: an agent's detached spawn
# process group (setsid → own session/pgid, recorded in the agent registry
# at spawn) can outlive the agent. Orphaned descendants keep holding
# cwd/open files inside a stale owned cache, so the disk-sentinel's
# "live process/cwd/open file" removal guard trips forever and the layer is
# never reaped (the original fix-wg-disk finding: 31 smoke-harness processes
# hung 11+ days blocking ~26 GB).
#
# Pre-fix behavior (reproduced on the base revision): the execute pass
# preserves the cache with "path has open files" forever because nothing
# ever reaps the dead agent's descendant group.
#
# Post-fix: the cleanup pass reaps the terminal agent's recorded orphaned
# process group (kill(-pgid), TERM then KILL) BEFORE the guard, the stale
# layer is removed in the same pass, and the ownership row retires.
# A RUNNING agent's descendant group is never reaped (second half).
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON" "python3 required for JSON assertions"
command -v setsid >/dev/null 2>&1 || loud_skip "MISSING SETSID" "setsid required to build the orphaned-group fixture"

scratch=$(make_scratch)
project="$scratch/project"
mkdir -p "$project"
cd "$project"
wg init --no-agency >/dev/null
cat > .wg/config.toml <<'EOF'
[agency]
auto_assign = false
auto_evaluate = false
EOF

# Build the owned cache: a stale target dir with enough body to matter.
owned_cache="$scratch/stale-orphan-group-cache"
mkdir -p "$owned_cache/baselines/$(printf 'd%.0s' $(seq 1 64))/target/debug"
dd if=/dev/zero of="$owned_cache/baselines/$(printf 'd%.0s' $(seq 1 64))/target/debug/lib.rmeta" bs=1024 count=8 status=none

# Orphan fixture: a process in its OWN session/process group (exactly what
# spawn's setsid() produces) with cwd inside the owned cache path. The
# session leader's PID is the pgid; `exec sleep` keeps the identity stable.
setsid bash -c 'cd "$1" && echo $$ > "$2" && exec sleep 300' _ "$owned_cache" "$scratch/orphan.pid" &
wait_for() {
  for _ in $(seq 1 50); do
    [ -s "$1" ] && return 0
    sleep 0.1
  done
  return 1
}
wait_for "$scratch/orphan.pid" || loud_fail "SETUP" "orphan fixture never recorded its pgid"
orphan_pid=$(cat "$scratch/orphan.pid")
kill -0 "$orphan_pid" 2>/dev/null || loud_fail "SETUP" "orphan fixture process $orphan_pid is not alive"

# A definitely-dead PID for the agent row: spawn, reap, and reuse the PID.
sh -c 'exit 0' &
dead_probe=$!
wait "$dead_probe" 2>/dev/null || true
dead_pid=$dead_probe
if kill -0 "$dead_pid" 2>/dev/null; then
  loud_fail "SETUP" "could not obtain a dead PID for the agent fixture"
fi

python3 - "$owned_cache" "$orphan_pid" "$dead_pid" <<'PY'
import json, os, sys
from pathlib import Path
cache = Path(sys.argv[1]).resolve()
orphan_pid = int(sys.argv[2])
dead_pid = int(sys.argv[3])
task_id = 'orphan-group-owner'
wg = Path('.wg').resolve()
task = {
    'kind': 'task', 'id': task_id,
    'title': 'stale cache owned by an agent with an orphaned descendant group',
    'status': 'done',
    'created_at': '2020-01-01T00:00:00Z',
    'completed_at': '2020-01-01T00:00:01Z',
}
(wg / 'graph.jsonl').write_text(json.dumps(task) + '\n')
os.makedirs(wg / 'service/disk', exist_ok=True)
registry = {
    'agents': {
        'agent-orphan-group': {
            'id': 'agent-orphan-group', 'pid': dead_pid,
            'pgid': orphan_pid,
            'task_id': task_id, 'executor': 'shell',
            'started_at': '2020-01-01T00:00:00Z',
            'last_heartbeat': '2020-01-01T00:00:00Z',
            'status': 'done', 'output_file': str(wg / 'agents/orphan/output.log'),
            'completed_at': '2020-01-01T00:00:01Z',
            'worktree_path': None,
        }
    },
    'next_agent_id': 100,
}
(wg / 'service/registry.json').write_text(json.dumps(registry))
ownership = {
    'schema': 1,
    'caches': [{
        'path': str(cache), 'kind': 'cargo-target',
        'task_id': task_id,
        'agent_id': 'agent-orphan-group', 'pid': dead_pid,
        'pid_start_epoch': None, 'mount_id': f'dev:{cache.stat().st_dev}',
        'created_at': '2020-01-01T00:00:00Z',
        'lease_expires_at': '2020-01-01T00:00:01Z',
    }],
}
(wg / 'service/disk/owned-caches.json').write_text(json.dumps(ownership))
PY

# Dry run: the guard must report the live descendant blocker (the orphan
# holds cwd inside the owned path) and must NOT kill anything.
dry=$(wg disk cleanup --json)
echo "$dry" | python3 -c '
import json, sys
report = json.load(sys.stdin)
assert report["considered"] == 1, report
assert report["reaped_groups"] == [], report
assert any("open file" in p["reason"] for p in report["preserved"]), report
' || loud_fail "dry run did not report the orphan-blocked guard: $dry"
kill -0 "$orphan_pid" 2>/dev/null || loud_fail "dry run killed the live descendant group (side effect in a read-only pass)"

# Execute: the orphaned descendant group of the terminal agent is reaped
# (kill(-pgid)) and the stale layer is removed in the same pass.
applied=$(wg disk cleanup --execute --json)
echo "$applied" | python3 -c '
import json, sys
report = json.load(sys.stdin)
assert report["reaped"] == 1, report
assert len(report["reaped_groups"]) == 1, report
assert "pgid:" in report["reaped_groups"][0]["path"], report
assert report["preserved"] == [], report
' || loud_fail "execute did not reap the orphaned group + stale layer: $applied"
[ ! -e "$owned_cache" ] \
  || loud_fail "stale owned cache survived the execute pass despite the reaped orphan group"
if kill -0 "$orphan_pid" 2>/dev/null; then
  loud_fail "orphaned descendant process $orphan_pid survived the terminal-owner group reap"
fi

# Idempotence: the ownership row retired; a second pass finds nothing.
second=$(wg disk cleanup --execute --json)
echo "$second" | python3 -c '
import json, sys
report = json.load(sys.stdin)
assert report["considered"] == 0, report
assert report["reaped"] == 0, report
' || loud_fail "second cleanup pass was not idempotent: $second"

# ── Negative half: a RUNNING agent's descendant group is never reaped ──
owned_live="$scratch/live-owner-cache"
mkdir -p "$owned_live"
setsid bash -c 'cd "$1" && echo $$ > "$2" && exec sleep 300' _ "$owned_live" "$scratch/live.pid" &
wait_for "$scratch/live.pid" || loud_fail "SETUP" "live-owner fixture never recorded its pgid"
live_pid=$(cat "$scratch/live.pid")

python3 - "$owned_live" "$live_pid" "$$" <<'PY'
import json, os, sys
from pathlib import Path
cache = Path(sys.argv[1]).resolve()
live_pid = int(sys.argv[2])
script_pid = int(sys.argv[3])
task_id = 'live-owner-task'
wg = Path('.wg').resolve()
task = {
    'kind': 'task', 'id': task_id,
    'title': 'running owner with live descendant group', 'status': 'in-progress',
    'created_at': '2020-01-01T00:00:00Z',
}
(wg / 'graph.jsonl').write_text(json.dumps(task) + '\n')
registry = json.loads((wg / 'service/registry.json').read_text())
# The agent itself is alive (our shell's ancestor chain), Working, and its
# recorded pgid owns the live fixture process.
registry['agents']['agent-live-owner'] = {
    'id': 'agent-live-owner', 'pid': script_pid,
    'pgid': live_pid,
    'task_id': task_id, 'executor': 'shell',
    'started_at': '2020-01-01T00:00:00Z',
    'last_heartbeat': '2020-01-01T00:00:00Z',
    'status': 'working', 'output_file': str(wg / 'agents/live/output.log'),
    'worktree_path': None,
}
(wg / 'service/registry.json').write_text(json.dumps(registry))
ownership = json.loads((wg / 'service/disk/owned-caches.json').read_text())
ownership['caches'].append({
    'path': str(cache), 'kind': 'cargo-target',
    'task_id': task_id,
    'agent_id': 'agent-live-owner', 'pid': 999999999,
    'pid_start_epoch': None, 'mount_id': f'dev:{cache.stat().st_dev}',
    'created_at': '2020-01-01T00:00:00Z',
    'lease_expires_at': '2020-01-01T00:00:01Z',
})
(wg / 'service/disk/owned-caches.json').write_text(json.dumps(ownership))
PY

live_applied=$(wg disk cleanup --execute --json)
echo "$live_applied" | python3 -c '
import json, sys
report = json.load(sys.stdin)
assert report["reaped_groups"] == [], report
assert report["reaped"] == 0, report
assert any("open file" in p["reason"] for p in report["preserved"]), report
' || loud_fail "a RUNNING agent's descendant group was touched: $live_applied"
[ -e "$owned_live" ] \
  || loud_fail "a running agent's owned layer was removed"
kill -0 "$live_pid" 2>/dev/null \
  || loud_fail "a running agent's live descendant process was killed"

echo "PASS: terminal agent's orphaned descendant group was reaped and the stale layer reaped in the same pass; the running agent's live descendant group was untouched"
