#!/usr/bin/env bash
# Fresh installs are graph-only until a human explicitly selects execution.
#
# Contract (docs/design-explicit-execution-system.md): a fresh WG has no active
# LLM execution system. Every LLM entry point MUST call one shared
# selection/readiness preflight and fail loudly with `WG-EXEC-UNSELECTED`
# before any fork/state/socket/worktree. The ratified production plane is Pi
# (the sole LLM handler); a selected route stays on its own execution system
# and never silently falls back to another handler.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

scratch=$(make_scratch)
cleanup_service() {
  env -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE HOME="$scratch/home" WG_GLOBAL_DIR="$scratch/global" \
    wg --dir "$scratch/project/.wg" service stop --force >/dev/null 2>&1 || true
}
add_cleanup_hook cleanup_service
mkdir -p "$scratch/home" "$scratch/global" "$scratch/project"

run_wg() {
  (cd "$scratch/project" && \
    env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE \
      HOME="$scratch/home" WG_GLOBAL_DIR="$scratch/global" \
      wg --dir "$scratch/project/.wg" "$@")
}

scenario_step() {
  printf 'SMOKE STEP %s: %s\n' "$1" "$2" >&2
}

# These bounded markers are normally hidden with the harness-owned logs. If a
# silent shell assertion fails, the captured stderr tail still identifies the
# last entered contract stage rather than reporting only `exit 1`.
scenario_step 1 'fresh graph-only init'
# 1. Fresh init is graph-only: graph exists, no route config written.
run_wg init --no-agency >"$scratch/init.out"
grep -q 'graph-only' "$scratch/init.out"
[[ -f "$scratch/project/.wg/graph.jsonl" ]]
[[ ! -f "$scratch/project/.wg/config.toml" ]]

scenario_step 2 'credential-free graph CRUD'
# 2. Graph CRUD is credential-free.
run_wg add 'graph-only task' >/dev/null 2>"$scratch/add.err"
run_wg publish graph-only-task --only >/dev/null
run_wg list | grep -q 'graph-only-task'

scenario_step 3 'unselected daemon admission'
# 3. A graph-only daemon may run, but a ready LLM task is refused at admission
#    before any attempt/claim/worktree. Human status names the exact missing
#    route and the supported project-local setup action.
scenario_step 3a 'starting graph-only service'
if ! run_wg service start --max-agents 1 --no-coordinator-agent --no-supervise \
  >"$scratch/unselected.out" 2>&1; then
  cat "$scratch/unselected.out" >&2
  loud_fail 'graph-only service start failed'
fi
scenario_step 3b 'waiting for route-missing status'
for _ in $(seq 1 100); do
  run_wg status >"$scratch/unselected-status.out" 2>&1
  grep -q 'WG-EXEC-ROUTE-MISSING' "$scratch/unselected-status.out" && break
  sleep 0.05
done
scenario_step 3c 'checking actionable route diagnostic'
grep -q 'WG-EXEC-ROUTE-MISSING' "$scratch/unselected-status.out"
grep -q 'wg setup --route pi' "$scratch/unselected-status.out"
if grep -qi 'falling back to claude\|falling back to pi\|default.*claude' "$scratch/unselected-status.out"; then
  echo 'FAIL: unselected status recommended an implicit fallback handler' >&2
  exit 1
fi
scenario_step 3d 'checking task stayed open'
run_wg show graph-only-task | grep -q 'Status: open'
scenario_step 3e 'checking no agent worktree exists'
[[ ! -d "$scratch/project/.wg/agents" ]]
scenario_step 3f 'stopping graph-only service'
run_wg service stop --force >/dev/null
scenario_step 3g 'graph-only service stopped'

scenario_step 4 'unselected manual spawn'
# 4. Manual worker spawn refuses without selection: the task stays open and no
#    agent worktree is created. `--executor pi` is a valid value that reaches
#    the selection preflight (non-Pi executors are rejected at arg parse).
if run_wg spawn graph-only-task --executor pi >"$scratch/spawn-unselected.out" 2>&1; then
  echo 'FAIL: manual worker spawn succeeded without selection' >&2
  exit 1
fi
grep -q 'WG-EXEC-UNSELECTED' "$scratch/spawn-unselected.out"
run_wg show graph-only-task | grep -q 'Status: open'
[[ ! -d "$scratch/project/.wg/agents" ]]

scenario_step 5 'unselected chat creation'
# 5. Chat creation refuses without selection; no chat row is persisted.
if run_wg chat create --name unselected-chat >"$scratch/chat-unselected.out" 2>&1; then
  echo 'FAIL: chat creation succeeded without selection' >&2
  exit 1
fi
grep -q 'WG-EXEC-UNSELECTED' "$scratch/chat-unselected.out"
! run_wg list | grep -q 'unselected-chat'

scenario_step 6 'interactive graph-only selection'
# 6. Drive the real interactive terminal wizard. The project-local cutover
#    removed the old scope prompt, so the route picker is now first and
#    defaults to Pi (recommended). Up moves to the adjacent graph-only
#    choice and Enter explicitly declines execution — no leading Enter,
#    which would otherwise accept the Pi default and descend into the model
#    wizard. A bounded `timeout` wrapper turns any future prompt drift into
#    a hard FAIL instead of an indefinite hang on exhausted stdin.
if ! printf '\033[A\n' | timeout 30s script -qec \
  "cd '$scratch/project' && env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE HOME='$scratch/home' WG_GLOBAL_DIR='$scratch/global' wg --dir '$scratch/project/.wg' setup" \
  "$scratch/setup-interactive.typescript" >/dev/null; then
  echo 'FAIL: interactive setup wizard did not return within 30s (prompt drift / stale key sequence)' >&2
  exit 1
fi
grep -q 'pi.*Pi (recommended)' "$scratch/setup-interactive.typescript"
grep -q 'Not now.*keep this WG graph-only' "$scratch/setup-interactive.typescript"
grep -q 'WG remains graph-only' "$scratch/setup-interactive.typescript"
[[ ! -f "$scratch/project/.wg/config.toml" ]]
[[ ! -f "$scratch/global/config.toml" ]]

scenario_step 7 'non-interactive missing route'
# 7. Non-interactive setup without an explicit route fails; it must not silently
#    select anything.
if run_wg setup --yes >"$scratch/setup-no-route.out" 2>&1; then
  echo 'FAIL: non-interactive setup silently selected a route' >&2
  exit 1
fi
grep -q 'route' "$scratch/setup-no-route.out"

scenario_step 8 'explicit project-local Pi selection'
# 8. Explicit Pi selection writes ONLY handler-first Pi routing. No implicit
#    Claude/Codex route may appear anywhere in the written config. The
#    project-local cutover moved the project config from `.wg/config.toml`
#    to `worksgood.toml` at the project root.
run_wg setup --route pi --scope local --yes \
  >"$scratch/setup-pi.out" 2>"$scratch/setup-pi.err"
grep -q 'model = "pi:openrouter:' "$scratch/project/worksgood.toml"
if grep -q 'model = "claude:' "$scratch/project/worksgood.toml"; then
  echo 'FAIL: explicit Pi setup wrote an implicit Claude route' >&2
  exit 1
fi
if grep -q 'model = "codex:' "$scratch/project/worksgood.toml"; then
  echo 'FAIL: explicit Pi setup wrote an implicit Codex route' >&2
  exit 1
fi
run_wg config lint --local >"$scratch/lint.out" 2>"$scratch/lint.err"
grep -q 'state: selected' "$scratch/lint.out"
grep -q 'route: pi:openrouter:' "$scratch/lint.out"

scenario_step 9 'selected Pi service lifecycle'
# 9. Real service lifecycle: the selected Pi handler reaches daemon startup
#    without silently changing systems. No tasks dispatch because max-agents=0.
scenario_step 9a 'starting selected Pi service'
if ! run_wg service start --max-agents 0 --no-coordinator-agent --no-supervise \
  >"$scratch/start-selected.out" 2>"$scratch/start-selected.err"; then
  cat "$scratch/start-selected.out" "$scratch/start-selected.err" >&2
  loud_fail 'selected Pi service start failed'
fi
scenario_step 9b 'waiting for selected Pi service status'
# `service start` establishes daemon readiness before returning, while the
# human status projection is a separate read. Poll it briefly just as the
# unselected admission leg above polls its asynchronous diagnostic, rather
# than making terminal scheduling decide whether this smoke passes.
selected_status_ready=0
for _ in $(seq 1 50); do
  if run_wg service status >"$scratch/status.out" 2>"$scratch/status.err" \
      && grep -q 'executor=pi' "$scratch/status.out"; then
    selected_status_ready=1
    break
  fi
  sleep 0.02
done
if [[ "$selected_status_ready" != 1 ]]; then
  cat "$scratch/status.out" "$scratch/status.err" >&2
  loud_fail 'selected service status did not become ready with executor=pi'
fi
if grep -qi 'executor=claude\|executor=codex' "$scratch/status.out" "$scratch/start-selected.out"; then
  echo 'FAIL: selected Pi daemon silently ran under a different executor' >&2
  exit 1
fi
scenario_step 9c 'stopping selected Pi service'
if ! run_wg service stop --force >/dev/null 2>"$scratch/stop.err"; then
  cat "$scratch/stop.err" >&2
  loud_fail 'selected Pi service stop failed'
fi
scenario_step 9d 'selected Pi service stopped'

echo 'PASS: fresh WG stayed graph-only, refused implicit dispatch, and honored explicit Pi selection without crossing systems'
