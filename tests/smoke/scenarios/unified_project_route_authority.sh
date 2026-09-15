#!/usr/bin/env bash
# Public-CLI regression for project-local route authority and inheritance.
set -euo pipefail

# The assigned integration target injects its exact candidate binary here so
# this same public-CLI scenario is part of the deterministic validation receipt.
if [[ -n "${WG_UNIFIED_ROUTE_TEST_BIN:-}" ]]; then
  export PATH="$(dirname "$WG_UNIFIED_ROUTE_TEST_BIN"):$PATH"
fi

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

scratch=$(make_scratch)
home="$scratch/home"
global="$scratch/global"
mkdir -p "$home" "$global"
# AF_UNIX sun_path is capped at 108 bytes. Some managed build roots are longer,
# so register one additional exact-owned short scratch for the daemon graph.
project=$(mktemp -d "/tmp/wgru-${BASHPID}.XXXXXX")
register_scratch "$project"
graph="$project/.wg"
export HOME="$home" WG_GLOBAL_DIR="$global"
cd "$project"

run_wg() {
  env -u WG_DIR -u WG_PROJECT_ROOT -u WG_TASK_ID -u WG_AGENT_ID \
    HOME="$home" WG_GLOBAL_DIR="$global" wg --dir "$graph" "$@"
}

run_wg init --no-agency >/dev/null
run_wg config set agent.model claude:route-a --no-reload >"$scratch/setup.out"

# One project default supplies both tiers until an operator deliberately splits one.
routes=$(run_wg config --models)
grep -q 'effective strong = claude:route-a' <<<"$routes"
grep -q 'effective weak   = claude:route-a' <<<"$routes"
grep -q 'active revision  = b3:' <<<"$routes"
grep -q 'inherited' <<<"$routes"

# Launch a supervised daemon under route A, mutate only the project authority to
# route B, then kill the daemon. The supervisor must respawn from current project
# bytes rather than replaying its stale --model argv.
start_wg_daemon "$project" --max-agents 0 --no-coordinator-agent
old_pid="$WG_SMOKE_DAEMON_PID"
run_wg config set agent.model claude:route-b --no-reload >"$scratch/change.out"
kill -TERM "$old_pid"

new_pid=""
for _ in $(seq 1 100); do
  if [[ -f "$graph/service/state.json" ]]; then
    candidate=$(python3 - "$graph/service/state.json" <<'PY'
import json,sys
try: print(json.load(open(sys.argv[1])).get("pid", ""))
except Exception: print("")
PY
)
    if [[ "$candidate" =~ ^[0-9]+$ ]] && [[ "$candidate" != "$old_pid" ]] \
      && kill -0 "$candidate" 2>/dev/null; then
      new_pid="$candidate"
      break
    fi
  fi
  sleep 0.2
done
[[ -n "$new_pid" ]] || loud_fail "supervisor did not respawn the daemon after project route change"
register_wg_daemon "$new_pid" "$graph"

run_wg service status --json >"$scratch/service-status.json"
python3 - "$scratch/service-status.json" <<'PY'
import json,sys
value=json.load(open(sys.argv[1]))
assert value["status"] == "running", value
assert value["coordinator"]["model"] == "claude:route-b", value
PY

run_wg status --json >"$scratch/status.json"
python3 - "$scratch/status.json" <<'PY'
import json,sys
value=json.load(open(sys.argv[1]))["coordinator"]
assert value["configured_model"] == "claude:route-b", value
assert value["effective_strong"]["route"] == "claude:route-b", value
assert value["effective_weak"]["route"] == "claude:route-b", value
assert value["effective_strong"]["provenance"] == "inherited", value
assert value["effective_weak"]["provenance"] == "inherited", value
assert value["config_source"] == "project-file", value
assert value["config_revision"].startswith("b3:"), value
PY

# Missing project authority and an unsupported Pi registration are different
# typed, attempt-neutral blockers. Mutate the project bytes while the same
# daemon stays alive and require the next bounded tick to report the new layer.
run_wg service stop --force >/dev/null
rm -rf "$graph/service"
cat >"$project/worksgood.toml" <<'TOML'
schema_version = 1

[models.default]
reasoning = "high"
TOML
run_wg add "route blocker remains attempt-neutral" --id route-blocker >/dev/null
run_wg publish route-blocker --only >/dev/null
start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --no-supervise

missing=0
for _ in $(seq 1 50); do
  run_wg service status --json >"$scratch/missing-status.json"
  if grep -q 'WG-EXEC-ROUTE-MISSING' "$scratch/missing-status.json"; then
    missing=1
    break
  fi
  sleep 0.2
done
[[ "$missing" == 1 ]] || loud_fail "missing project route did not surface in service status"
run_wg show route-blocker --json >"$scratch/missing-task.json"
python3 - "$scratch/missing-task.json" <<'PY'
import json,sys
value=json.load(open(sys.argv[1]))
assert value["status"] == "open", value
assert value.get("dispatch_count", 0) == 0, value
assert value.get("spawn_failures", 0) == 0, value
PY

cat >"$project/worksgood.toml" <<'TOML'
schema_version = 1

[models.default]
model = "pi:openai-codex:gpt-5.6-sol"
TOML
reasoning_missing=0
for _ in $(seq 1 50); do
  run_wg service status --json >"$scratch/reasoning-status.json"
  if grep -q 'WG-EXEC-REASONING-MISSING' "$scratch/reasoning-status.json"; then
    reasoning_missing=1
    break
  fi
  sleep 0.2
done
[[ "$reasoning_missing" == 1 ]] || loud_fail "Pi route without reasoning was not blocked before admission"
run_wg show route-blocker --json >"$scratch/reasoning-task.json"
python3 - "$scratch/reasoning-task.json" <<'PY'
import json,sys
value=json.load(open(sys.argv[1]))
assert value["status"] == "open", value
assert value.get("dispatch_count", 0) == 0, value
assert value.get("spawn_failures", 0) == 0, value
PY

cat >"$project/worksgood.toml" <<'TOML'
schema_version = 1

[models.default]
model = "pi:definitely-not-registered:fixture"
reasoning = "high"
TOML
unsupported=0
for _ in $(seq 1 50); do
  run_wg service status --json >"$scratch/unsupported-status.json"
  if grep -q 'WG-PI-PROVIDER-UNSUPPORTED' "$scratch/unsupported-status.json"; then
    unsupported=1
    break
  fi
  sleep 0.2
done
[[ "$unsupported" == 1 ]] || loud_fail "unsupported Pi route did not replace the missing-route blocker without restart"
run_wg show route-blocker --json >"$scratch/unsupported-task.json"
python3 - "$scratch/unsupported-task.json" <<'PY'
import json,sys
value=json.load(open(sys.argv[1]))
assert value["status"] == "open", value
assert value.get("dispatch_count", 0) == 0, value
assert value.get("spawn_failures", 0) == 0, value
PY

# Direct review/agency calls share the hermetic capability barrier rather than
# reaching Pi and discovering the unsupported provider during a model call.
printf 'ordinary reviewed content\n' >"$scratch/review-input.txt"
WG_REVIEW_MODEL=1 run_wg review check \
  --class IC1 --trust verified --author local:test --sensitivity high \
  --content-file "$scratch/review-input.txt" \
  >"$scratch/review.out" 2>"$scratch/review.err" || true
grep -q 'WG-PI-PROVIDER-UNSUPPORTED' "$scratch/review.err"
grep -q 'review-unavailable' "$scratch/review.out"
run_wg service stop --force >/dev/null

# Exercise the supported reset-to-inherit surface. This edits the existing Pi
# profile definition and rematerializes the selected project document.
run_wg profile select pi --no-reload >"$scratch/select-pi.out"
run_wg profile pi --weak openai-codex/gpt-5.6-sol >"$scratch/set-weak.out"
grep -q 'explicit' "$scratch/set-weak.out"
run_wg profile pi --reset-weak >"$scratch/reset-weak.out"
grep -q 'reset → inherit strong' "$scratch/reset-weak.out"
reset_routes=$(run_wg config --models)
grep -q 'effective weak   = .*\[inherited:' <<<"$reset_routes"
! grep -q '^fast = ' "$project/worksgood.toml"

echo "PASS: project revision owns supervised respawn; tiers inherit; weak reset rematerializes"
