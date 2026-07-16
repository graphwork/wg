#!/usr/bin/env bash
# Regression: a TUI launched from a worker must pin its background `wg`
# commands to the TUI graph. Before fix-integration-test, first-use chat
# bootstrap used `Command::new("wg")` plus current_dir only. An inherited
# WG_DIR won over that CWD and created the chat task in the live parent graph.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
if ! command -v tmux >/dev/null 2>&1; then
    loud_skip "MISSING TMUX" "tmux not on PATH; cannot drive the real TUI bootstrap"
fi

parent=$(make_scratch)
scratch=$(make_scratch)
session="wgsmoke-tui-isolation-$$"
cleanup_tui_isolation() {
    tmux kill-session -t "$session" 2>/dev/null || true
}
add_cleanup_hook cleanup_tui_isolation

env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID wg --dir "$parent/.wg" init --route claude-cli >/dev/null 2>&1 \
    || loud_fail "failed to initialize sentinel parent graph"
env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID wg --dir "$scratch/.wg" init --route claude-cli >/dev/null 2>&1 \
    || loud_fail "failed to initialize scratch TUI graph"

parent_graph="$parent/.wg/graph.jsonl"
scratch_graph="$scratch/.wg/graph.jsonl"
parent_before=$(sha256sum "$parent_graph")

# Drive the actual TUI startup path in a PTY. The top-level --dir deliberately
# points at scratch, while the inherited worker routing deliberately points at
# parent. The asynchronous first-use bootstrap must create `.chat-0` in scratch.
tmux new-session -d -s "$session" -x 120 -y 36 \
    "cd '$scratch' && env TERM=xterm-256color WG_USER=tui-isolation WG_DIR='$parent/.wg' WG_TASK_ID=sentinel-parent-task WG_AGENT_ID=sentinel-parent-agent wg --dir '$scratch/.wg' tui --no-mouse"

created=0
for _ in $(seq 1 80); do
    if grep -q '"id":"\.chat-0"' "$scratch_graph" 2>/dev/null; then
        created=1
        break
    fi
    sleep 0.1
done

if [[ "$created" -ne 1 ]]; then
    parent_tail=$(tail -n 5 "$parent_graph" 2>/dev/null || true)
    scratch_tail=$(tail -n 5 "$scratch_graph" 2>/dev/null || true)
    loud_fail "TUI first-use chat did not land in scratch graph. parent=${parent_tail} scratch=${scratch_tail}"
fi

parent_after=$(sha256sum "$parent_graph")
if [[ "$parent_before" != "$parent_after" ]]; then
    loud_fail "TUI bootstrap mutated sentinel parent graph: before=${parent_before} after=${parent_after} rows=$(cat "$parent_graph")"
fi
if grep -q '"id":"\.coordinator' "$parent_graph"; then
    loud_fail "legacy .coordinator ghost appeared in sentinel parent graph"
fi
if grep -q '"id":"\.chat-' "$parent_graph"; then
    loud_fail "chat bootstrap task leaked into sentinel parent graph"
fi
if grep -q '"id":"\.coordinator' "$scratch_graph"; then
    loud_fail "new TUI bootstrap created a legacy .coordinator task instead of .chat-0"
fi

echo "PASS: real TUI bootstrap pinned its built-wg child to scratch; sentinel parent hash stayed unchanged and no legacy coordinator ghost was created"
