#!/usr/bin/env bash
# Scenario: wg_msg_send_shows_indicator_in_viz
#
# Regression: the TUI message-indicator (✉ next to a task in the task list)
# stayed stale until something else mutated graph.jsonl. `wg msg send` only
# writes to messages/<task>.jsonl, so the fs watcher's graph-mtime trigger
# missed it and the indicator disappeared / never appeared.
#
# Live smoke: send a message via the CLI and assert that `wg viz` (which
# shares its rendering path with the TUI's task list) produces the ✉
# indicator. This is a fast read-only check — no daemon, no LLM.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

if ! wg init -x shell >init.log 2>&1; then
    loud_fail "wg init failed during smoke setup: $(tail -5 init.log)"
fi

if ! wg add "Smoke task" --id smoke-task >>init.log 2>&1; then
    loud_fail "wg add failed: $(tail -5 init.log)"
fi

# Send from a non-coordinator-side sender so coordinator_message_status
# treats it as incoming. (The default sender is "user" / WG_TASK_ID, which
# is correctly classified as coordinator-side outgoing — different code
# path; the per-row indicator path being tested here counts ANY message.)
if ! wg msg send smoke-task --from agent-smoke "hi" >msg.log 2>&1; then
    loud_fail "wg msg send failed: $(cat msg.log)"
fi

# `wg viz` shares its rendering pipeline with the TUI's task list — both
# call generate_viz_output_from_graph, which builds the per-row indicator.
if ! NO_COLOR=1 wg viz >viz.log 2>&1; then
    loud_fail "wg viz failed: $(cat viz.log)"
fi

if ! grep -q "smoke-task" viz.log; then
    loud_fail "wg viz output missing smoke-task row:\n$(cat viz.log)"
fi

# The ✉ glyph must appear on the smoke-task row.
if ! grep "smoke-task" viz.log | grep -q $'\xe2\x9c\x89'; then
    loud_fail "message indicator (✉) missing from smoke-task row after wg msg send:\n$(cat viz.log)"
fi

echo "PASS: ✉ indicator appears on task row after wg msg send"
exit 0
