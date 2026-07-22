#!/usr/bin/env bash
# Real CLI + TUI regression for add-disk-sentinel. Thresholds are synthesized
# relative to the measured free bytes; the test never fills the real disk.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON" "python3 required for JSON assertions"

scratch=$(make_scratch)
project="$scratch/project"
mkdir -p "$project"
cd "$project"
wg init --no-agency >/dev/null
wg add "disk sentinel visible row cargo test" --no-place \
  --exec "printf resumed > '$project/resumed'" >/dev/null
wg add "stale owned cache evidence" --no-place >/dev/null
wg done stale-owned-cache >/dev/null

write_cfg() {
  cat > .wg/config.toml <<EOF
[agency]
auto_assign = false
auto_evaluate = false

[dispatcher.resource_management]
disk_sentinel_enabled = true
disk_warning_bytes = $1
disk_pause_build_bytes = $2
disk_hard_refuse_bytes = $3
disk_warning_percent = 0.0
disk_pause_build_percent = 0.0
disk_hard_refuse_percent = 0.0
disk_resume_hysteresis_bytes = 0
disk_resume_hysteresis_percent = 0.0
disk_scan_interval_seconds = 1
max_build_agents = 1
# This smoke isolates reactive warning cleanup. The 40–60 GiB projection and
# concurrent reservation are covered sparsely by the Rust companion test.
estimated_build_bytes = 0
estimated_build_heavy_bytes = 0
build_link_test_safety_bytes = 0
stream_retention_days = 0
EOF
}

write_cfg 0 0 0
healthy=$(wg disk doctor --json)
free=$(printf '%s' "$healthy" | python3 -c 'import json,sys; print(min(m["free_bytes"] for m in json.load(sys.stdin)["mounts"]))')
[ "$free" -gt 1024 ] || loud_fail "invalid synthetic baseline free=$free"
low=$((free + 1))

write_cfg "$low" 0 0
warning=$(wg disk doctor --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["level"])')
[ "$warning" = "warning" ] || loud_fail "expected warning, got $warning"
write_cfg "$low" "$low" 0
pause=$(wg disk doctor --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["level"])')
[ "$pause" = "pause-builds" ] || loud_fail "expected pause-builds, got $pause"
write_cfg "$low" "$low" "$low"
refuse=$(wg disk doctor --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["level"])')
[ "$refuse" = "hard-refuse" ] || loud_fail "expected hard-refuse, got $refuse"

# wg status consumes the bounded cache rather than walking the synthetic target.
status_level=$(wg status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["disk"]["level"])')
[ "$status_level" = "hard-refuse" ] || loud_fail "status did not expose cached hard-refuse: $status_level"

# Recovery with zero thresholds proves automatic hysteresis release without disk filling.
write_cfg 0 0 0
resumed=$(wg disk doctor --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["level"])')
[ "$resumed" = "healthy" ] || loud_fail "expected automatic recovery, got $resumed"

# A filename is never ownership. The dry-run/apply surfaces must preserve an
# unknown /tmp/wg-target-* directory.
unknown="/tmp/wg-target-unknown-smoke-$$"
mkdir -p "$unknown"; printf unknown > "$unknown/blob"
wg disk cleanup --execute --json > "$scratch/cleanup.json"
[ -f "$unknown/blob" ] || loud_fail "unknown /tmp/wg-target-* was deleted"
rm -rf "$unknown"

# Real TUI human flow: hard pressure must not block graph browsing/input. The
# approved symbolic context bar intentionally carries no disk payload; `status`
# above is the detailed cached surface, while the TUI remains responsive.
if command -v tmux >/dev/null 2>&1; then
  write_cfg "$low" "$low" "$low"
  wg disk doctor >/dev/null
  session="wg-disk-smoke-$$"
  tmux new-session -d -s "$session" -x 160 -y 40 "cd '$project' && wg tui"
  pane=$(tmux list-panes -t "$session" -F '#{pane_id}' | head -1)
  [ -n "$pane" ] || loud_fail "TUI tmux pane did not start"
  sleep 3
  screen=$(tmux capture-pane -p -t "$pane" -S -80 || true)
  dump=$(wg tui-dump 2>/dev/null || true)
  screen="$screen
$dump"
  tmux has-session -t "$session" 2>/dev/null \
    || loud_fail "TUI exited or wedged under synthetic hard-refuse"
  echo "$screen" | grep -q 'disk-sentinel-visible' \
    || loud_fail "TUI did not render the graph under hard-refuse; screen=$(printf '%q' "$screen")"
  tmux send-keys -t "$pane" q >/dev/null 2>&1 || true
  tmux kill-session -t "$session" >/dev/null 2>&1 || true
else
  echo "TUI assertion skipped locally: tmux unavailable" >&2
fi

# End-to-end pressure/recovery: allocate a modest, explicitly-owned cache and
# put the mount just below pause. The daemon must reclaim that cache at warning
# pressure, preserve unrelated dirty source, refresh hysteresis, and complete
# the already-waiting heavy task without a config rewrite or ENOSPC.
dirty_source="$project/valuable-dirty-source"
mkdir -p "$dirty_source"
printf 'uncommitted source must survive\n' > "$dirty_source/valuable.rs"
owned_cache="$scratch/owned-stale-target"
mkdir -p "$owned_cache"
python3 - "$owned_cache/blob" <<'PY'
import os, sys
with open(sys.argv[1], 'wb') as f:
    os.posix_fallocate(f.fileno(), 0, 512 * 1024 * 1024)
os.sync()
PY
pressure_free=$(wg disk doctor --json | python3 -c 'import json,sys; print(min(m["free_bytes"] for m in json.load(sys.stdin)["mounts"]))')
pressure_floor=$((pressure_free + 256 * 1024 * 1024))
mkdir -p .wg/service/disk .wg/service
python3 - "$owned_cache" "$dirty_source" <<'PY'
import json, os, sys
from pathlib import Path
cache = Path(sys.argv[1]).resolve()
source = Path(sys.argv[2]).resolve()
wg = Path('.wg').resolve()
registry = {
    'agents': {
        'agent-stale-disk-smoke': {
            'id': 'agent-stale-disk-smoke', 'pid': 999999999,
            'task_id': 'stale-owned-cache', 'executor': 'shell',
            'started_at': '2020-01-01T00:00:00Z',
            'last_heartbeat': '2020-01-01T00:00:00Z',
            'status': 'done', 'output_file': str(wg / 'agents/stale/output.log'),
            'completed_at': '2020-01-01T00:00:01Z',
            'worktree_path': str(source),
        }
    },
    'next_agent_id': 100,
}
(wg / 'service/registry.json').write_text(json.dumps(registry))
ownership = {
    'schema': 1,
    'caches': [{
        'path': str(cache), 'kind': 'temporary',
        'task_id': 'stale-owned-cache',
        'agent_id': 'agent-stale-disk-smoke', 'pid': 999999999,
        'pid_start_epoch': None, 'mount_id': f'dev:{cache.stat().st_dev}',
        'created_at': '2020-01-01T00:00:00Z',
        'lease_expires_at': '2020-01-01T00:00:01Z',
        'worktree_path': str(source),
    }],
}
(wg / 'service/disk/owned-caches.json').write_text(json.dumps(ownership))
PY
write_cfg "$pressure_floor" "$pressure_floor" 0
level=$(wg disk doctor --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["level"])')
[ "$level" = pause-builds ] || loud_fail "owned-cache pressure did not reach pause-builds: $level"
[ ! -e "$project/resumed" ] \
  || loud_fail "build-heavy task ran before the pressure fixture existed"
start_wg_daemon "$project" --max-agents 1 --no-chat-agent
for _ in $(seq 1 120); do
  [ ! -e "$owned_cache" ] && break
  sleep 0.25
done
[ ! -e "$owned_cache" ] \
  || loud_fail "daemon did not proactively reclaim the stale explicitly-owned cache"
[ -f "$dirty_source/valuable.rs" ] \
  || loud_fail "cache-only cleanup altered dirty source"
grep -q 'uncommitted source must survive' "$dirty_source/valuable.rs" \
  || loud_fail "dirty source content changed during cache cleanup"
for _ in $(seq 1 120); do
  [ -e "$project/resumed" ] && break
  sleep 0.25
done
[ -e "$project/resumed" ] \
  || loud_fail "waiting build did not complete after automatic safe cleanup; disk=$(wg disk doctor --json 2>&1); daemon=$(tail -80 "$project/.wg/service/daemon.log" 2>&1); graph=$(wg list 2>&1)"
grep -q 'Disk cleanup: reaped 1 owned target' "$project/.wg/service/daemon.log" \
  || loud_fail "daemon did not report the automatic cache decision: $(tail -40 "$project/.wg/service/daemon.log")"
status=$(python3 - <<'PY'
import json
for row in open('.wg/graph.jsonl'):
    obj=json.loads(row)
    if obj.get('title') == 'disk sentinel visible row cargo test':
        print(obj.get('status'))
        break
PY
)
[ "$status" = done ] || loud_fail "recovered shell task status=$status, expected done"

echo "PASS: warning pressure automatically reaped only owned cache, preserved dirty source, and completed waiting build without ENOSPC; cached status/TUI and unknown-target safety hold"
