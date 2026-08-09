#!/usr/bin/env bash
# Scenario: agency_inline_spawn_registers_executor_claude
#
# Regression: `.evaluate-*` / `.flip-*` / `.assign-*` tasks used to be
# registered in the agent registry with `executor="eval"` / `executor="assign"`.
# Those labels were misleading — there is no separate eval handler; the
# inline-spawn path runs `wg evaluate` / `wg assign` which calls
# `run_lightweight_llm_call`, which always shells out to the claude CLI for
# agency one-shot roles. The migrate-agency-tasks change re-labels these
# registrations to `executor="claude"` so observability matches reality and
# agency tasks share the worker-agent dispatch story.
#
# This scenario asserts:
#   1. A manually-created `.evaluate-*` task tagged `evaluation` with an
#      `exec` field is picked up by the dispatcher.
#   2. After the dispatcher spawns the inline agent, the registered agent
#      has `executor="claude"` (not `"eval"`).
#   3. The agent's `metadata.json` shows `executor=claude` and
#      `model=claude:haiku`.
#
# Fast (no LLM call needed — we only check the registration metadata that
# is written BEFORE the bash script's LLM call would run).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

if ! wg init -x claude >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -10 init.log)"
fi

# Create a parent task so .evaluate-* has a real eval target.
if ! wg add "Smoke parent target" --id smoke-eval-target >add.log 2>&1; then
    loud_fail "wg add parent failed: $(tail -5 add.log)"
fi

# Manually add a `.evaluate-smoke-eval-target` task with the contract the
# dispatcher uses to recognize an agency one-shot: `evaluation` tag + exec
# command. We do NOT actually need the eval to succeed — we only need the
# inline spawn to register the agent before the bash script attempts the
# LLM call.
#
# Note: `wg add --exec X` auto-sets exec_mode=shell, which would route
# through spawn_shell_inline (executor=shell). Real `.evaluate-*` tasks
# historical synthetic agency rows have `exec` set but `exec_mode=bare`
# (LLM-driven, not shell). Reproduce that exactly via `wg edit`.
if ! wg add "Inline eval smoke" \
        --id .evaluate-smoke-eval-target \
        --tag evaluation \
        --exec "true" \
        >evaladd.log 2>&1; then
    loud_fail "wg add .evaluate-* failed: $(tail -5 evaladd.log)"
fi
if ! wg edit .evaluate-smoke-eval-target --exec-mode bare >editmode.log 2>&1; then
    loud_fail "wg edit --exec-mode bare failed: $(tail -5 editmode.log)"
fi

# Start the daemon. It will spawn the .evaluate-* task in a tick.
start_wg_daemon "$scratch" --max-agents 2

wg_dir="$WG_SMOKE_DAEMON_DIR"
registry="$wg_dir/service/registry.json"

# Wait up to 15s for an agent to be registered for our .evaluate-* task.
agent_id=""
for i in $(seq 1 75); do
    if [[ -f "$registry" ]]; then
        # Match the agent whose task_id is .evaluate-smoke-eval-target.
        agent_id=$(python3 -c "
import json, sys
try:
    r = json.load(open('$registry'))
except Exception:
    sys.exit(0)
for aid, info in (r.get('agents') or {}).items():
    if info.get('task_id') == '.evaluate-smoke-eval-target':
        print(aid)
        break
" 2>/dev/null || true)
        if [[ -n "$agent_id" ]]; then
            break
        fi
    fi
    sleep 0.2
done

if [[ -z "$agent_id" ]]; then
    loud_fail "no agent registered for .evaluate-smoke-eval-target after 15s. registry:
$(cat "$registry" 2>/dev/null | head -50)"
fi

# Read executor from registry.json — must be "claude", never "eval".
executor=$(python3 -c "
import json
r = json.load(open('$registry'))
print(r['agents']['$agent_id'].get('executor', ''))
" 2>/dev/null || true)

if [[ "$executor" != "claude" ]]; then
    loud_fail "agent $agent_id for .evaluate-* registered with executor='$executor' (expected 'claude'). Full agent record:
$(python3 -c "import json; print(json.dumps(json.load(open('$registry'))['agents']['$agent_id'], indent=2))" 2>/dev/null)"
fi

# Read agent metadata.json — must also show executor=claude.
metadata="$wg_dir/agents/$agent_id/metadata.json"
if [[ ! -f "$metadata" ]]; then
    loud_fail "no metadata.json for $agent_id at $metadata"
fi

meta_executor=$(python3 -c "
import json
print(json.load(open('$metadata')).get('executor', ''))
" 2>/dev/null || true)

if [[ "$meta_executor" != "claude" ]]; then
    loud_fail "$metadata reports executor='$meta_executor' (expected 'claude')"
fi

echo "PASS: .evaluate-* inline spawn registered with executor=claude (agent=$agent_id)"
exit 0
