#!/usr/bin/env bash
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

init_out="$scratch/init.out"
wg init -m claude:opus --no-agency >"$init_out" 2>&1 || \
    loud_fail "wg init failed: $(tail -20 "$init_out")"

chat_out="$scratch/chat-opencode.out"
if wg chat new --name opencode-pane --exec opencode --model claude:opus >"$chat_out" 2>&1; then
    loud_fail "wg chat new --exec opencode unexpectedly succeeded"
fi
chat_msg="$(cat "$chat_out")"
echo "$chat_msg" | grep -qF "opencode" || \
    loud_fail "chat create error should name opencode, got: $chat_msg"
echo "$chat_msg" | grep -qF "worker-only" || \
    loud_fail "chat create error should explain worker-only boundary, got: $chat_msg"
echo "$chat_msg" | grep -qF "live chat executor" || \
    loud_fail "chat create error should name the live-chat path, got: $chat_msg"

if grep -qE '"id":"\.chat-[0-9]+"' "$scratch/.wg/graph.jsonl"; then
    loud_fail "rejected worker-only live chat created a .chat task: $(cat "$scratch/.wg/graph.jsonl")"
fi

spawn_out="$scratch/spawn-task-opencode.out"
if WG_EXECUTOR_TYPE=opencode WG_MODEL=claude:opus \
    wg spawn-task --dry-run .coordinator-0 >"$spawn_out" 2>&1; then
    loud_fail "wg spawn-task --dry-run with opencode unexpectedly succeeded"
fi
spawn_msg="$(cat "$spawn_out")"
echo "$spawn_msg" | grep -qF "opencode" || \
    loud_fail "spawn-task error should name opencode, got: $spawn_msg"
echo "$spawn_msg" | grep -qF "worker-only" || \
    loud_fail "spawn-task error should explain worker-only boundary, got: $spawn_msg"
echo "$spawn_msg" | grep -qF "spawn-task/live chat" || \
    loud_fail "spawn-task error should name the rejected path, got: $spawn_msg"

echo "PASS: worker-only external executors are rejected from live chat and spawn-task paths"
