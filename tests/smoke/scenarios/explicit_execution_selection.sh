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

scratch=$(mktemp -d)
trap 'env -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE HOME="$scratch/home" WG_GLOBAL_DIR="$scratch/global" wg --dir "$scratch/project/.wg" service stop --force >/dev/null 2>&1 || true; rm -rf "$scratch"' EXIT
mkdir -p "$scratch/home" "$scratch/global" "$scratch/project"

run_wg() {
  (cd "$scratch/project" && \
    env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE \
      HOME="$scratch/home" WG_GLOBAL_DIR="$scratch/global" \
      wg --dir "$scratch/project/.wg" "$@")
}

# 1. Fresh init is graph-only: graph exists, no route config written.
run_wg init --no-agency >"$scratch/init.out"
grep -q 'graph-only' "$scratch/init.out"
[[ -f "$scratch/project/.wg/graph.jsonl" ]]
[[ ! -f "$scratch/project/.wg/config.toml" ]]

# 2. Graph CRUD is credential-free.
run_wg add 'graph-only task' >/dev/null 2>"$scratch/add.err"
run_wg list | grep -q 'graph-only-task'

# 3. service start refuses without explicit selection: structured
#    WG-EXEC-UNSELECTED error naming the supported route, and NO daemon state
#    (state.json / socket / claim / worktree) is created.
if run_wg service start --no-coordinator-agent >"$scratch/unselected.out" 2>&1; then
  echo 'FAIL: service start succeeded without explicit execution selection' >&2
  exit 1
fi
grep -q 'WG-EXEC-UNSELECTED' "$scratch/unselected.out"
# The unselected block must name the supported (Pi) route and never recommend
# a different handler as an automatic repair.
grep -q 'wg setup --route pi' "$scratch/unselected.out"
grep -q 'wg profile select pi' "$scratch/unselected.out"
if grep -qi 'falling back to claude\|falling back to pi\|default.*claude' "$scratch/unselected.out"; then
  echo 'FAIL: unselected error recommended an implicit fallback handler' >&2
  exit 1
fi
[[ ! -e "$scratch/project/.wg/service/state.json" ]]
[[ ! -e "$scratch/project/.wg/service/daemon.sock" ]]

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

# 5. Chat creation refuses without selection; no chat row is persisted.
if run_wg chat create --name unselected-chat >"$scratch/chat-unselected.out" 2>&1; then
  echo 'FAIL: chat creation succeeded without selection' >&2
  exit 1
fi
grep -q 'WG-EXEC-UNSELECTED' "$scratch/chat-unselected.out"
! run_wg list | grep -q 'unselected-chat'

# 6. Drive the real interactive terminal wizard. Enter accepts the scope prompt;
#    setup intentionally recommends Pi on a fresh graph, so Up moves to the
#    adjacent graph-only choice and Enter explicitly declines execution.
printf '\n\033[A\n' | script -qec \
  "cd '$scratch/project' && env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE HOME='$scratch/home' WG_GLOBAL_DIR='$scratch/global' wg --dir '$scratch/project/.wg' setup" \
  "$scratch/setup-interactive.typescript" >/dev/null
grep -q 'pi.*Pi (recommended)' "$scratch/setup-interactive.typescript"
grep -q 'Not now.*keep this WG graph-only' "$scratch/setup-interactive.typescript"
grep -q 'WG remains graph-only' "$scratch/setup-interactive.typescript"
[[ ! -f "$scratch/project/.wg/config.toml" ]]
[[ ! -f "$scratch/global/config.toml" ]]

# 7. Non-interactive setup without an explicit route fails; it must not silently
#    select anything.
if run_wg setup --yes >"$scratch/setup-no-route.out" 2>&1; then
  echo 'FAIL: non-interactive setup silently selected a route' >&2
  exit 1
fi
grep -q 'route' "$scratch/setup-no-route.out"

# 8. Explicit Pi selection writes ONLY handler-first Pi routing. No implicit
#    Claude/Codex route may appear anywhere in the written config.
run_wg setup --route pi --scope local --yes \
  >"$scratch/setup-pi.out" 2>"$scratch/setup-pi.err"
grep -q 'model = "pi:openrouter:' "$scratch/project/.wg/config.toml"
if grep -q 'model = "claude:' "$scratch/project/.wg/config.toml"; then
  echo 'FAIL: explicit Pi setup wrote an implicit Claude route' >&2
  exit 1
fi
if grep -q 'model = "codex:' "$scratch/project/.wg/config.toml"; then
  echo 'FAIL: explicit Pi setup wrote an implicit Codex route' >&2
  exit 1
fi
run_wg config lint --local >"$scratch/lint.out" 2>"$scratch/lint.err"
grep -q 'state: selected' "$scratch/lint.out"
grep -q 'route: pi:openrouter:' "$scratch/lint.out"

# 9. Real service lifecycle: the selected Pi handler reaches daemon startup
#    without silently changing systems. No tasks dispatch because max-agents=0.
run_wg service start --max-agents 0 --no-coordinator-agent \
  >"$scratch/start-selected.out" 2>"$scratch/start-selected.err"
run_wg service status >"$scratch/status.out" 2>"$scratch/status.err"
grep -q 'executor=pi' "$scratch/status.out"
if grep -qi 'executor=claude\|executor=codex' "$scratch/status.out" "$scratch/start-selected.out"; then
  echo 'FAIL: selected Pi daemon silently ran under a different executor' >&2
  exit 1
fi
run_wg service stop --force >/dev/null 2>"$scratch/stop.err"

echo 'PASS: fresh WG stayed graph-only, refused implicit dispatch, and honored explicit Pi selection without crossing systems'
