#!/usr/bin/env bash
# Safe default plus visible emergency override for predictive build admission.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON" "python3 required for JSON assertions"

scratch=$(make_scratch)
export HOME="$scratch/home"
export WG_GLOBAL_DIR="$HOME/.wg"
export XDG_CONFIG_HOME="$HOME/.config"
mkdir -p "$HOME" "$XDG_CONFIG_HOME"

make_project() {
  local project="$1" mode="$2"
  mkdir -p "$project"
  cd "$project"
  git init -q -b main
  git -c user.name='WG Smoke' -c user.email='wg@example.invalid' commit --allow-empty -qm base
  wg init --no-agency >/dev/null
  git add .gitignore AGENTS.md CLAUDE.md
  git -c user.name='WG Smoke' -c user.email='wg@example.invalid' commit -qm wg-init
  cat > .wg/config.toml <<EOF
[agency]
auto_assign = false
auto_evaluate = false

[dispatcher]
poll_interval = 1
settling_delay_ms = 0

[dispatcher.resource_management]
cargo_target_root = "$project-cache"
disk_warning_bytes = 0
disk_pause_build_bytes = 0
disk_hard_refuse_bytes = 0
disk_warning_percent = 0.0
disk_pause_build_percent = 0.0
disk_hard_refuse_percent = 0.0
build_link_test_safety_bytes = 0
estimated_build_bytes = 0
estimated_build_heavy_bytes = 0
estimated_cargo_baseline_bytes = 0
max_build_agents = 1
EOF
  if [ "$mode" = disabled ]; then
    printf '\ndisk_sentinel_enabled = false\n' >> .wg/config.toml
  fi
  # Service selection is explicit even though fixture workers are shell-only.
  wg config --local --model pi:openrouter:test/fake --no-reload >/dev/null
  mkdir -p .wg/service/disk
  cat > .wg/service/disk/build-high-water.json <<'EOF'
{"schema":2,"build_capable_delta_bytes":18446744073709551615,"build_heavy_delta_bytes":18446744073709551615}
EOF
}

# Absent key: safe default refuses before workspace/attempt creation. Persisted
# status names intentional scheduler backpressure rather than a wedge.
default_project="$scratch/default"
make_project "$default_project" default
wg add "default protected cargo test" --id default-protected-cargo-test \
  --exec "printf launched > '$default_project/ran'; wg wait \"\$WG_TASK_ID\" --until message --checkpoint recovered" >/dev/null
wg publish default-protected-cargo-test --only >/dev/null
default_lint=$(wg config lint --local)
echo "$default_lint" | grep -q 'state: enabled (safe default)' \
  || loud_fail "config lint did not expose enabled safe default: $default_lint"
start_wg_daemon "$default_project" --max-agents 1 --no-chat-agent --interval 1
for _ in $(seq 1 80); do
  state=$(wg service status --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("coordinator",{}).get("dispatch_state",""))' 2>/dev/null || true)
  [ "$state" = admission-deferred ] && break
  sleep 0.25
done
[ "${state:-}" = admission-deferred ] \
  || loud_fail "safe default did not expose admission deferral: $(wg service status --json 2>&1)"
[ ! -e "$default_project/ran" ] || loud_fail "safe default launched through impossible private-delta reserve"
[ ! -d "$default_project/.wg-worktrees" ] \
  || [ -z "$(find "$default_project/.wg-worktrees" -mindepth 1 -print -quit 2>/dev/null)" ] \
  || loud_fail "admission refusal created a worktree/attempt before spawn"
human_status=$(wg service status)
echo "$human_status" | grep -q 'Admission deferred:' \
  || loud_fail "human status did not distinguish admission refusal: $human_status"
echo "$human_status" | grep -q 'dispatcher is not wedged' \
  || loud_fail "human status omitted non-wedge diagnosis: $human_status"
sleep 6
! grep -q 'dispatcher appears wedged' "$default_project/.wg/service/daemon.log" \
  || loud_fail "watchdog mislabeled safe admission deferral as wedged"
# Restore headroom without touching the task. The same published source must
# dispatch exactly once; the refused probe was scheduler state, not an attempt.
cat > "$default_project/.wg/service/disk/build-high-water.json" <<'EOF'
{"schema":2,"build_capable_delta_bytes":0,"build_heavy_delta_bytes":0}
EOF
rm -f "$default_project/.wg/service/disk/disk-sentinel.json"
wg service reload >/dev/null
for _ in $(seq 1 120); do
  [ -e "$default_project/ran" ] && break
  sleep 0.25
done
[ -e "$default_project/ran" ] \
  || loud_fail "recovered headroom did not dispatch the deferred source exactly once: status=$(wg service status --json 2>&1); highwater=$(cat .wg/service/disk/build-high-water.json 2>&1); daemon=$(tail -100 .wg/service/daemon.log 2>&1)"
python3 - "$default_project" <<'PY'
import json, sys
from pathlib import Path
p = Path(sys.argv[1])
rows = [json.loads(line) for line in (p/'.wg/graph.jsonl').read_text().splitlines() if line.strip()]
task = next(row for row in rows if row.get('id') == 'default-protected-cargo-test')
assert task.get('lifecycle', {}).get('attempt_sequence') == 1, task
registry = json.loads((p/'.wg/service/registry.json').read_text())
agents = [a for a in registry.get('agents', {}).values() if a.get('task_id') == task['id']]
assert len(agents) == 1, agents
PY

# Explicit false remains an emergency escape hatch, is loud in lint output, and
# does not mutate preserved source/artifacts. Removing the impossible reserve is
# an operator decision; WG never silently rewrites this explicit override.
override_project="$scratch/override"
make_project "$override_project" disabled
mkdir -p "$override_project/preserved-source"
printf 'valuable dirty recovery source\n' > "$override_project/preserved-source/dirty.rs"
wg add "emergency override cargo test" --id emergency-override-cargo-test \
  --exec "printf launched > '$override_project/launched'; wg wait \"\$WG_TASK_ID\" --until message --checkpoint override" >/dev/null
wg publish emergency-override-cargo-test --only >/dev/null
override_lint=$(wg config lint --local || true)
echo "$override_lint" | grep -q 'state: disabled (explicit emergency override)' \
  || loud_fail "config lint hid explicit override: $override_lint"
echo "$override_lint" | grep -q 'restore safe default:' \
  || loud_fail "config lint omitted safe-default remediation: $override_lint"
start_wg_daemon "$override_project" --max-agents 1 --no-chat-agent --interval 1
for _ in $(seq 1 80); do
  [ -e "$override_project/launched" ] && break
  sleep 0.25
done
[ -e "$override_project/launched" ] \
  || loud_fail "explicit emergency override did not launch: $(tail -80 "$override_project/.wg/service/daemon.log" 2>&1)"
grep -q 'valuable dirty recovery source' "$override_project/preserved-source/dirty.rs" \
  || loud_fail "explicit override flow altered preserved source"

printf '%s\n' "PASS: predictive private-delta admission is safe by default, refuses before attempt creation, exposes non-wedge backpressure, and keeps an explicit emergency override loud"
