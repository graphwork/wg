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

# --- aider is STILL worker-only: rejected from live chat + spawn-task ---------
# (OpenCode used to be here too, but fix-opencode-build made it chat-capable;
# the remaining external CLIs have no live chat handler and must still reject.)
chat_out="$scratch/chat-aider.out"
if wg chat new --name aider-pane --exec aider --model claude:opus >"$chat_out" 2>&1; then
    loud_fail "wg chat new --exec aider unexpectedly succeeded"
fi
chat_msg="$(cat "$chat_out")"
echo "$chat_msg" | grep -qF "aider" || \
    loud_fail "chat create error should name aider, got: $chat_msg"
echo "$chat_msg" | grep -qF "worker-only" || \
    loud_fail "chat create error should explain worker-only boundary, got: $chat_msg"
echo "$chat_msg" | grep -qF "live chat executor" || \
    loud_fail "chat create error should name the live-chat path, got: $chat_msg"

if grep -qE '"id":"\.chat-[0-9]+"' "$scratch/.wg/graph.jsonl"; then
    loud_fail "rejected worker-only live chat created a .chat task: $(cat "$scratch/.wg/graph.jsonl")"
fi

spawn_out="$scratch/spawn-task-aider.out"
if WG_EXECUTOR_TYPE=aider WG_MODEL=claude:opus \
    wg spawn-task --dry-run .coordinator-0 >"$spawn_out" 2>&1; then
    loud_fail "wg spawn-task --dry-run with aider unexpectedly succeeded"
fi
spawn_msg="$(cat "$spawn_out")"
echo "$spawn_msg" | grep -qF "aider" || \
    loud_fail "spawn-task error should name aider, got: $spawn_msg"
echo "$spawn_msg" | grep -qF "worker-only" || \
    loud_fail "spawn-task error should explain worker-only boundary, got: $spawn_msg"
echo "$spawn_msg" | grep -qF "spawn-task/live chat" || \
    loud_fail "spawn-task error should name the rejected path, got: $spawn_msg"

# --- opencode IS chat-capable (fix-opencode-build, goal #5) -------------------
# Creating a live chat with --exec opencode must succeed and write a .chat task.
oc_route="opencode:openrouter/stepfun/step-3.7-flash"
oc_chat_out="$scratch/chat-opencode.out"
if ! wg chat new --name opencode-pane --exec opencode --model "$oc_route" \
        >"$oc_chat_out" 2>&1; then
    loud_fail "wg chat new --exec opencode should now succeed, got: $(cat "$oc_chat_out")"
fi
if ! grep -qE '"id":"\.chat-[0-9]+"' "$scratch/.wg/graph.jsonl"; then
    loud_fail "opencode live chat did not create a .chat task: $(cat "$scratch/.wg/graph.jsonl")"
fi

# And spawn-task must dispatch the chat via `wg opencode-handler --chat` with the
# resolved model passed EXPLICITLY (never opencode's internal default).
oc_spawn_out="$scratch/spawn-task-opencode.out"
if ! WG_EXECUTOR_TYPE=opencode WG_MODEL="$oc_route" \
        wg spawn-task --dry-run .coordinator-0 >"$oc_spawn_out" 2>&1; then
    loud_fail "wg spawn-task --dry-run with opencode should now succeed, got: $(cat "$oc_spawn_out")"
fi
oc_spawn_msg="$(cat "$oc_spawn_out")"
echo "$oc_spawn_msg" | grep -qF "opencode-handler" || \
    loud_fail "spawn-task should dispatch to wg opencode-handler, got: $oc_spawn_msg"
echo "$oc_spawn_msg" | grep -qF "openrouter:stepfun/step-3.7-flash" || \
    loud_fail "spawn-task preview must carry the resolved model explicitly, got: $oc_spawn_msg"
if grep -qiE 'claude-handler' <<<"$oc_spawn_msg"; then
    loud_fail "opencode chat must NOT fall back to the claude handler: $oc_spawn_msg"
fi

echo "PASS: aider/etc. stay worker-only-rejected; opencode dispatches via opencode-handler with an explicit model"
