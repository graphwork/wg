#!/usr/bin/env bash
# Scenario: agent_guide_warns_off_wg_nex
#
# Regression: chat agents interpreted `wg nex` as a one-shot LLM dispatch
# command. It is an interactive REPL; when launched from a non-PTY bash
# subprocess it hangs on stdin and freezes the chat turn.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

out="$(wg agent-guide 2>&1)" || loud_fail "wg agent-guide failed"

grep -q "Don't run wg nex from bash" <<<"$out" || \
    loud_fail "agent guide missing explicit wg nex bash warning"
grep -q "interactive REPL" <<<"$out" || \
    loud_fail "agent guide does not describe wg nex as interactive"
grep -q "hang on stdin" <<<"$out" || \
    loud_fail "agent guide does not name the stdin hang failure mode"
grep -q 'wg add "description" --after <current-task-id>' <<<"$out" || \
    loud_fail "agent guide does not direct agents to file subtasks via wg add"
grep -q "wg evaluate run <task>" <<<"$out" || \
    loud_fail "agent guide does not mention batch-mode evaluation command"

echo "PASS: wg agent-guide warns agents not to run wg nex from bash"
exit 0
