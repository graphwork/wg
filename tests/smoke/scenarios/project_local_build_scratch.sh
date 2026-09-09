#!/usr/bin/env bash
# Candidate-binary terminal regression: a daemon launched outside the selected
# project must give build-capable work project-local scratch and report that
# exact write surface in disk doctor JSON.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$(command -v wg)}"
[ -x "$WG_BIN" ] || loud_fail "candidate wg is not executable: $WG_BIN"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON" "python3 required for JSON assertions"

# This fixture models a human launching a selected project, not the worker that
# happens to execute the smoke gate.
unset WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_REASONING WG_TIER WG_SPAWN_EPOCH \
  WG_WORKER_CAPABILITY WG_WORKER_CONTROL_PROTOCOL WG_WORKER_IPC \
  WG_WORKER_CONTROL_MODE WG_WORKER_GENERATION WG_WORKER_ATTEMPT_ID \
  WG_WORKER_ATTEMPT_FENCE WG_GRAPH_ID WG_SPAWN_RUN_ID || true
# Unix-domain sockets have a short path cap; an agent's inherited TMPDIR may be
# deeply nested, so use the harness's conventional short root unless provided.
export WG_SMOKE_ROOT="${WG_SMOKE_ROOT:-/tmp/wgsmoke}"

scratch=$(make_scratch)
fakebin="$scratch/bin"
project="$scratch/selected-project"
launcher="$scratch/unrelated-launch-cwd"
mkdir -p "$fakebin" "$project" "$launcher"
ln -s "$WG_BIN" "$fakebin/wg"
export PATH="$fakebin:$PATH"
(
  cd "$project"
  wg init --no-agency >/dev/null
)
cat > "$project/.wg/config.toml" <<'EOF'
[agency]
auto_assign = false
auto_evaluate = false

[dispatcher]
worktree_isolation = false

[dispatcher.resource_management]
disk_sentinel_enabled = true
disk_warning_bytes = 0
disk_pause_build_bytes = 0
disk_hard_refuse_bytes = 0
disk_warning_percent = 0.0
disk_pause_build_percent = 0.0
disk_hard_refuse_percent = 0.0
disk_resume_hysteresis_bytes = 0
disk_resume_hysteresis_percent = 0.0
estimated_build_bytes = 0
estimated_build_heavy_bytes = 0
estimated_cargo_baseline_bytes = 0
build_link_test_safety_bytes = 0
EOF

observed="$project/observed-tmpdir"
release="$project/release-probe"
wg --dir "$project" add "cargo test project-local TMPDIR" \
  --id project-local-tmpdir-probe \
  --exec-mode shell \
  --exec "printf '%s\n' \"\$TMPDIR\" > '$observed'; while [ ! -e '$release' ]; do sleep 0.1; done" >/dev/null
wg --dir "$project" publish project-local-tmpdir-probe --only >/dev/null

# Exercise the real detached daemon from a cwd unrelated to the graph. The
# helper still reads and registers the canonical daemon PID for safe teardown.
WG_SMOKE_DAEMON_LAUNCH_CWD="$launcher"
export WG_SMOKE_DAEMON_LAUNCH_CWD
start_wg_daemon "$project" --max-agents 1 --no-chat-agent

for _ in $(seq 1 120); do
  [ -s "$observed" ] && break
  sleep 0.25
done
[ -s "$observed" ] \
  || loud_fail "build-capable task never recorded TMPDIR; wrapper=$(tail -40 "$project/daemon.log" 2>/dev/null || true); daemon=$(tail -100 "$project/.wg/service/daemon.log" 2>/dev/null || true); graph=$(wg --dir "$project" list 2>&1 || true)"

tmpdir=$(cat "$observed")
expected_prefix="$project/.wg/build-tmp/agent-"
case "$tmpdir" in
  "$expected_prefix"*) ;;
  *) loud_fail "spawned TMPDIR is not beside selected project: got=$tmpdir expected-prefix=$expected_prefix" ;;
esac
legacy_prefix="${TMPDIR:-/tmp}/wg/build-tmp/"
case "$tmpdir" in
  "$legacy_prefix"*) loud_fail "new default allocation still used legacy OS temp root: $tmpdir" ;;
esac

doctor="$scratch/doctor.json"
(
  cd "$launcher"
  wg --dir "$project" disk doctor --json > "$doctor"
)
python3 - "$doctor" "$project/.wg/build-tmp" "${TMPDIR:-/tmp}" <<'PY'
import json, os, sys
snapshot = json.load(open(sys.argv[1]))
expected = os.path.realpath(sys.argv[2])
os_tmp = os.path.realpath(sys.argv[3])
probes = [p for mount in snapshot['mounts'] for p in mount.get('probes', [])]
matching = [p for p in probes if p.get('source') == 'project-build-scratch']
if not matching:
    raise SystemExit(f"missing project-build-scratch probe: {probes!r}")
if not any(os.path.realpath(p['path']) == expected for p in matching):
    raise SystemExit(f"scratch probe did not identify selected project {expected}: {matching!r}")
if any(os.path.realpath(p['path']) == os_tmp for p in probes):
    raise SystemExit(f"unused OS temp directory remained an admission probe: {probes!r}")
if any(p.get('source') == 'legacy-owned-build-scratch' for p in probes):
    raise SystemExit(f"fresh project unexpectedly reported legacy scratch: {probes!r}")
# The containing mount may also host other project paths, but its authoritative
# logical probes must include the exact selected-project scratch write surface.
mount = next(m for m in snapshot['mounts'] if any(
    p.get('source') == 'project-build-scratch' and os.path.realpath(p['path']) == expected
    for p in m.get('probes', [])
))
if not mount.get('mount_id'):
    raise SystemExit(f"scratch admission mount lacked identity: {mount!r}")
PY
human="$scratch/doctor.txt"
(
  cd "$launcher"
  wg --dir "$project" disk doctor > "$human"
)
grep -F "probe project-build-scratch: $project/.wg/build-tmp" "$human" >/dev/null \
  || loud_fail "human disk doctor did not identify project scratch path/source: $(cat "$human")"

touch "$release"

echo "PASS: real daemon launched from unrelated cwd used selected-project .wg/build-tmp TMPDIR; disk doctor reports project scratch and omits unused OS temp"
