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
wg add "disk sentinel visible row" --no-place >/dev/null

write_cfg() {
  cat > .wg/config.toml <<EOF
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

# Real TUI human flow: async worker reads the small cached snapshot and the
# status bar renders it. No filesystem scan runs in input/render.
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
  tmux send-keys -t "$pane" q >/dev/null 2>&1 || true
  tmux kill-session -t "$session" >/dev/null 2>&1 || true
  echo "$screen" | grep -q 'disk HardRefuse' || loud_fail "TUI status bar did not render cached disk HardRefuse; screen=$(printf '%q' "$screen")"
else
  echo "TUI assertion skipped locally: tmux unavailable" >&2
fi

echo "PASS: synthetic disk admission warn/pause/refuse/resume, cached status/TUI, unknown target preservation"
