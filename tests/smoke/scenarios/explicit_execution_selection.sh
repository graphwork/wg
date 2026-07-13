#!/usr/bin/env bash
# Fresh installs are graph-only until a human explicitly selects execution.
set -euo pipefail

scratch=$(mktemp -d)
trap 'env -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE HOME="$scratch/home" WG_GLOBAL_DIR="$scratch/global" wg --dir "$scratch/project/.wg" service stop --force >/dev/null 2>&1 || true; rm -rf "$scratch"' EXIT
mkdir -p "$scratch/home" "$scratch/global" "$scratch/project"

run_wg() {
  (cd "$scratch/project" && \
    env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE \
      HOME="$scratch/home" WG_GLOBAL_DIR="$scratch/global" \
      wg --dir "$scratch/project/.wg" "$@")
}

run_wg init --no-agency >"$scratch/init.out"
grep -q 'graph-only' "$scratch/init.out"
[[ -f "$scratch/project/.wg/graph.jsonl" ]]
[[ ! -f "$scratch/project/.wg/config.toml" ]]

run_wg add 'graph-only task' >/dev/null 2>"$scratch/add.err"
run_wg list | grep -q 'graph-only-task'

if run_wg service start --no-coordinator-agent >"$scratch/unselected.out" 2>&1; then
  echo 'FAIL: service start succeeded without explicit execution selection' >&2
  exit 1
fi
grep -q 'WG-EXEC-UNSELECTED' "$scratch/unselected.out"
grep -q 'wg setup --route codex-cli --yes' "$scratch/unselected.out"
grep -q 'wg profile use <name>' "$scratch/unselected.out"
[[ ! -e "$scratch/project/.wg/service/state.json" ]]
[[ ! -e "$scratch/project/.wg/service/daemon.sock" ]]
if run_wg spawn graph-only-task --executor claude >"$scratch/spawn-unselected.out" 2>&1; then
  echo 'FAIL: manual worker spawn succeeded without selection' >&2
  exit 1
fi
grep -q 'WG-EXEC-UNSELECTED' "$scratch/spawn-unselected.out"
run_wg show graph-only-task | grep -q 'Status: open'
[[ ! -d "$scratch/project/.wg/agents" ]]
if run_wg chat create --name unselected-chat >"$scratch/chat-unselected.out" 2>&1; then
  echo 'FAIL: chat creation succeeded without selection' >&2
  exit 1
fi
grep -q 'WG-EXEC-UNSELECTED' "$scratch/chat-unselected.out"
! run_wg list | grep -q 'unselected-chat'

# Drive the real interactive terminal wizard. Enter accepts the scope prompt;
# the fresh route picker must default to graph-only rather than detected Claude.
printf '\n\n' | script -qec \
  "cd '$scratch/project' && env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE HOME='$scratch/home' WG_GLOBAL_DIR='$scratch/global' wg --dir '$scratch/project/.wg' setup" \
  "$scratch/setup-interactive.typescript" >/dev/null
grep -q 'Not now.*keep this WG graph-only' "$scratch/setup-interactive.typescript"
grep -q 'WG remains graph-only' "$scratch/setup-interactive.typescript"
[[ ! -f "$scratch/project/.wg/config.toml" ]]
[[ ! -f "$scratch/global/config.toml" ]]

if run_wg setup --yes >"$scratch/setup-no-route.out" 2>&1; then
  echo 'FAIL: non-interactive setup silently selected a route' >&2
  exit 1
fi
grep -q 'requires an explicit route' "$scratch/setup-no-route.out"

run_wg setup --route codex-cli --scope local --yes \
  >"$scratch/setup-codex.out" 2>"$scratch/setup-codex.err"
grep -q 'model = "codex:gpt-5.5"' "$scratch/project/.wg/config.toml"
if grep -q 'model = "claude:' "$scratch/project/.wg/config.toml"; then
  echo 'FAIL: explicit Codex setup wrote an implicit Claude route' >&2
  exit 1
fi
run_wg config lint --local >"$scratch/lint.out" 2>"$scratch/lint.err"
grep -q 'state: selected' "$scratch/lint.out"
grep -q 'route: codex:gpt-5.5' "$scratch/lint.out"
# The legacy-executor migration diagnostic remains active, but is contained in
# the scenario artifact rather than leaking control bytes into the parent TUI.
grep -q 'executor.*deprecated' "$scratch/lint.err"

# Real service lifecycle: selected handler reaches daemon startup without
# silently changing systems. No tasks dispatch because max-agents=0.
run_wg service start --max-agents 0 --no-coordinator-agent \
  >"$scratch/start-selected.out" 2>"$scratch/start-selected.err"
run_wg service status >"$scratch/status.out" 2>"$scratch/status.err"
grep -q 'codex' "$scratch/status.out" "$scratch/start-selected.out"
run_wg service stop --force >/dev/null 2>"$scratch/stop.err"

echo 'PASS: fresh WG stayed graph-only, refused implicit dispatch, and honored explicit Codex selection'
