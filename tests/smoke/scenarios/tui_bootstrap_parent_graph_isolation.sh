#!/usr/bin/env bash
# Regression: a TUI launched from a worker must pin its background `wg`
# commands to the TUI graph. Before fix-integration-test, the New-chat
# launcher used `Command::new("wg")` plus current_dir only. An inherited WG_DIR
# won over that CWD and created the explicitly requested chat in the live parent graph.

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
scratch_before=$(sha256sum "$scratch_graph")

# Drive the actual TUI startup path in a PTY. The top-level --dir deliberately
# points at scratch, while inherited worker routing deliberately points at
# parent. Opening alone is graph-only: no child command and no chat creation.
tmux new-session -d -s "$session" -x 120 -y 36 \
    "cd '$scratch' && env TERM=xterm-256color WG_USER=tui-isolation WG_DIR='$parent/.wg' WG_TASK_ID=sentinel-parent-task WG_AGENT_ID=sentinel-parent-agent wg --dir '$scratch/.wg' tui --no-mouse"

ready=0
for _ in $(seq 1 80); do
    if tmux capture-pane -p -t "$session" 2>/dev/null | grep -q 'No chat selected'; then
        ready=1
        break
    fi
    sleep 0.1
done
[[ "$ready" -eq 1 ]] || loud_fail "empty scratch TUI did not render No chat selected"
[[ "$(sha256sum "$scratch_graph")" == "$scratch_before" ]] \
    || loud_fail "opening the TUI mutated the scratch graph before explicit input"
[[ "$(sha256sum "$parent_graph")" == "$parent_before" ]] \
    || loud_fail "opening the TUI mutated the inherited parent graph"

# Explicit command-mode n is the creation boundary. Confirm the selected preset
# with Enter; the isolated child must land in scratch, never the inherited graph.
tmux send-keys -t "$session" n
for _ in $(seq 1 40); do
    tmux capture-pane -p -t "$session" 2>/dev/null | grep -q 'New Chat' && break
    sleep 0.1
done
tmux send-keys -t "$session" Enter
created=0
for _ in $(seq 1 80); do
    if grep -q '"id":"\.chat-0"' "$scratch_graph" 2>/dev/null; then
        created=1
        break
    fi
    sleep 0.1
done
[[ "$created" -eq 1 ]] || loud_fail "explicit n/Enter did not create .chat-0 in scratch"

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

echo "PASS: TUI open was non-mutating; explicit n/Enter pinned its wg child to scratch; sentinel parent stayed unchanged"
