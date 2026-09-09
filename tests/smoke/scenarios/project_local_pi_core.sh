#!/usr/bin/env bash
# Project execution must not inherit machine-global WG routing.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
scratch=$(make_scratch)
mkdir -p "$scratch/home" "$scratch/global" "$scratch/project"

run_wg() {
  (cd "$scratch/project" && env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE \
    HOME="$scratch/home" WG_GLOBAL_DIR="$scratch/global" \
    wg --dir "$scratch/project/.wg" "$@")
}

# Initialize and prove graph CRUD before any machine route exists.
run_wg init --no-agency >"$scratch/init.out"
run_wg add "graph-only survives stale global config" >"$scratch/add.out"

# Simulate routing left behind by another project, including a live-looking
# active profile pointer and unrelated global capacity.
cat >"$scratch/global/config.toml" <<'TOML'
[agent]
model = "pi:machine-global:stale-route"

[dispatcher]
model = "pi:machine-global:stale-route"
max_agents = 99

[models.task_agent]
model = "pi:machine-global:stale-route"
reasoning = "high"
TOML
printf 'stale-pi\n' >"$scratch/global/active-profile"

run_wg list >"$scratch/list.out"
grep -q "graph-only" "$scratch/list.out"

# The real service entry point must refuse before daemon/session/claim/worktree
# state is created, while naming both ignored machine inputs as inactive.
if run_wg service start --no-coordinator-agent >"$scratch/start.out" 2>&1; then
  echo "FAIL: service inherited machine-global routing" >&2
  exit 1
fi
grep -q "WG-EXEC-UNSELECTED" "$scratch/start.out"
grep -q "Ignored legacy routing" "$scratch/start.out"
grep -q "config.toml" "$scratch/start.out"
grep -q "active-profile" "$scratch/start.out"
[[ ! -e "$scratch/project/.wg/service/state.json" ]]
[[ ! -e "$scratch/project/.wg/service/daemon.sock" ]]
[[ ! -d "$scratch/project/.wg/agents" ]]

# The single-tick/debug path reaches real evaluation lanes and must enforce the
# same preflight before it can claim or persist anything. Graph bytes stay exact.
graph_before=$(cksum "$scratch/project/.wg/graph.jsonl")
if run_wg service tick >"$scratch/tick.out" 2>&1; then
  loud_fail "service tick accepted stale machine-global routing"
fi
grep -q "WG-EXEC-UNSELECTED" "$scratch/tick.out"
[[ "$graph_before" == "$(cksum "$scratch/project/.wg/graph.jsonl")" ]]
[[ ! -d "$scratch/project/.wg/agents" ]]

# Global non-routing capacity is also not inherited; the source-aware project
# view reports the built-in value rather than 99/global.
run_wg config get dispatcher.max_agents --json >"$scratch/config.json"
! grep -q '"value":99' "$scratch/config.json"
grep -q 'builtin-default' "$scratch/config.json"

# A checked-in route cannot self-authorize a host executable. Until the
# digest-bound operator ceiling is present, the request fails before service
# or worker state and the hostile binary is never invoked.
cat >"$scratch/project/worksgood.toml" <<'TOML'
schema_version = 1
[agent]
model = "pi:test:worker"
[bash]
path = "/tmp/hostile-project-bash"
TOML
if run_wg service start --no-coordinator-agent >"$scratch/host-auth.out" 2>&1; then
  loud_fail "project bash.path started a service without operator authorization"
fi
grep -q "WG-PROJECT-AUTHORIZATION-REQUIRED" "$scratch/host-auth.out"
[[ ! -e "$scratch/project/.wg/service/state.json" ]]
[[ ! -d "$scratch/project/.wg/agents" ]]

echo "PASS: stale global config/profile stayed inactive; graph-only CRUD and host authorization stayed fail-closed"
