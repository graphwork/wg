#!/usr/bin/env bash
# Availability-first default plus explicit predictive-admission/status regression.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON" "python3 required for JSON assertions"

scratch=$(make_scratch)

make_project() {
  local project="$1" enabled="$2"
  mkdir -p "$project"
  cd "$project"
  wg init --no-agency >/dev/null
  cat > .wg/config.toml <<EOF
[agency]
auto_assign = false
auto_evaluate = false

[dispatcher]
poll_interval = 1
settling_delay_ms = 0

[dispatcher.resource_management]
disk_sentinel_enabled = $enabled
disk_warning_bytes = 0
disk_pause_build_bytes = 0
disk_hard_refuse_bytes = 0
disk_warning_percent = 0.0
disk_pause_build_percent = 0.0
disk_hard_refuse_percent = 0.0
build_link_test_safety_bytes = 0
max_build_agents = 1
EOF
  mkdir -p .wg/service/disk
  cat > .wg/service/disk/build-high-water.json <<'EOF'
{"build_capable_bytes":18446744073709551615,"build_heavy_bytes":18446744073709551615}
EOF
}

# Default/false: even an impossible historical cold-build projection has no
# admission authority. A preserved dirty warm target is not charged again and
# must remain byte-for-byte present.
default_project="$scratch/default"
make_project "$default_project" false
mkdir -p "$default_project/.wg-worktrees/preserved-recovery/target"
printf 'valuable dirty recovery source\n' > "$default_project/.wg-worktrees/preserved-recovery/dirty.rs"
printf 'warm target evidence\n' > "$default_project/.wg-worktrees/preserved-recovery/target/warm"
# A hard-refuse snapshot left by an older opt-in is observation only once the
# gate is disabled; it must not retain stale dispatch authority.
cat > "$default_project/.wg/service/disk/disk-sentinel.json" <<EOF
{"schema":1,"generated_at":"2020-01-01T00:00:00Z","level":"hard-refuse","reason":"stale prior opt-in refusal","mounts":[],"targets":[],"worktrees":{"path":"$default_project/.wg-worktrees","bytes":0,"complete":true},"agents":{"path":"$default_project/.wg/agents","bytes":0,"complete":true},"log":{"path":"$default_project/.wg/log","bytes":0,"complete":true},"active_builds":0,"active_build_heavy":0,"projected_headroom_bytes":0}
EOF
wg add "default historical cargo test" --id default-historical-cargo-test \
  --exec "printf launched > '$default_project/launched'" >/dev/null
wg publish default-historical-cargo-test --only >/dev/null
default_lint=$(wg config lint --local)
echo "$default_lint" | grep -q 'state: disabled (default)' \
  || loud_fail "config lint did not explain disabled default: $default_lint"
start_wg_daemon "$default_project" --max-agents 1 --no-chat-agent --interval 1
for _ in $(seq 1 80); do
  [ -e "$default_project/launched" ] && break
  sleep 0.25
done
[ -e "$default_project/launched" ] \
  || loud_fail "default-disabled admission blocked process creation: $(tail -80 "$default_project/.wg/service/daemon.log" 2>&1)"
grep -q 'valuable dirty recovery source' "$default_project/.wg-worktrees/preserved-recovery/dirty.rs" \
  || loud_fail "dirty recovery source was altered"
grep -q 'warm target evidence' "$default_project/.wg-worktrees/preserved-recovery/target/warm" \
  || loud_fail "preserved warm target was deleted or altered"

# Explicit true: the same high-water remains a deterministic advanced opt-in
# refusal. Persisted status must name intentional admission deferral, and the
# watchdog must never call that dispatcher wedged.
optin_project="$scratch/optin"
make_project "$optin_project" true
wg add "opt in historical cargo test" --id opt-in-historical-cargo-test \
  --exec "printf should-not-run > '$optin_project/ran'" >/dev/null
wg publish opt-in-historical-cargo-test --only >/dev/null
optin_lint=$(wg config lint --local)
echo "$optin_lint" | grep -q 'state: enabled (advanced explicit opt-in)' \
  || loud_fail "config lint did not identify explicit opt-in: $optin_lint"
echo "$optin_lint" | grep -q 'set dispatcher.resource_management.disk_sentinel_enabled = false' \
  || loud_fail "config lint omitted migration guidance: $optin_lint"
start_wg_daemon "$optin_project" --max-agents 1 --no-chat-agent --interval 1
for _ in $(seq 1 80); do
  state=$(wg service status --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("coordinator",{}).get("dispatch_state",""))' 2>/dev/null || true)
  [ "$state" = admission-deferred ] && break
  sleep 0.25
done
[ "${state:-}" = admission-deferred ] \
  || loud_fail "service status did not expose intentional admission deferral: $(wg service status --json 2>&1)"
[ ! -e "$optin_project/ran" ] || loud_fail "explicit predictive opt-in did not refuse the projected build"
human_status=$(wg service status)
echo "$human_status" | grep -q 'Admission deferred:' \
  || loud_fail "human status did not distinguish admission refusal: $human_status"
echo "$human_status" | grep -q 'dispatcher is not wedged' \
  || loud_fail "human status omitted non-wedge diagnosis: $human_status"
# Cross the five-tick watchdog threshold and prove intentional deferral resets it.
sleep 6
! grep -q 'dispatcher appears wedged' "$optin_project/.wg/service/daemon.log" \
  || loud_fail "watchdog mislabeled opt-in admission deferral as wedged: $(tail -80 "$optin_project/.wg/service/daemon.log")"
grep -q "Deferring 'opt-in-historical-cargo-test'.*build admission paused" "$optin_project/.wg/service/daemon.log" \
  || loud_fail "daemon log did not identify intentional admission refusal"

echo "PASS: predictive build admission is availability-first by default, deterministic when opted in, preserves warm recovery state, and status never mislabels refusal as a wedge"
