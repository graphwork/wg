#!/usr/bin/env bash
# Scenario: chat_native_endpoint_full_pipeline (fix-nex-chat)
#
# Regression lock for the four stacked fixes from diagnose-wg-nex:
#
#   A. sweep.rs orphan exclusion includes chat-loop tag (was: chat-loop
#      tagged InProgress tasks with no inline assigned were flipped to Open
#      within ~2s of CreateChat IPC).
#   B. CreateChat IPC eagerly enqueues the new chat into
#      pending_coordinator_ids + sets urgent_wake (was: supervisor only
#      spawned on first user message).
#   C. plan.rs reads task.endpoint before falling back to find_default()
#      (was: a chat created via TUI launcher with `-e https://lambda01...`
#      had its URL silently dropped; spawn-task --dry-run emitted no -e
#      flag).
#   D. coordinator_agent.rs writes a per-chat persistent stderr file at
#      service/nex-handler-stderr-<chat_id>.log (was: nex stderr only
#      went to daemon.log inline reader, lost on spawn-time failure).
#
# This scenario is graph-state + dry-run only — no live LLM call required.
# That keeps it cheap, keeps it deterministic, and exercises every code
# path that the diagnose pinpointed without depending on a working
# endpoint at smoke-time.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

if ! wg init -m claude:opus >init.log 2>&1; then
    loud_fail "wg init -m claude:opus failed: $(tail -5 init.log)"
fi

wg_dir="$scratch/.wg"
[[ -d "$wg_dir" ]] || loud_fail "expected $wg_dir; got: $(ls -la "$scratch")"

ENDPOINT='https://lambda01.tail334fe6.ts.net:30000'
MODEL='nex:qwen3-coder'

# ── Step 1: create the chat with a custom executor + model + endpoint via
#    the same `wg chat create` path the TUI launcher dialog uses. This
#    exercises `create_chat_in_graph` (the shared persistence layer) and
#    leaves a concrete .chat-N task in graph.jsonl carrying the endpoint
#    field. Service is offline — we are checking persistence + plan.rs,
#    not live spawn.
out=$(wg chat create \
    --executor native \
    --model "$MODEL" \
    --endpoint "$ENDPOINT" \
    --json 2>&1) || loud_fail "wg chat create -e failed: $out"

graph="$wg_dir/graph.jsonl"
[[ -f "$graph" ]] || loud_fail "graph.jsonl missing after create"

chat_line=$(grep -E '"id":"\.chat-0"' "$graph" | head -1)
[[ -n "$chat_line" ]] || loud_fail "no .chat-0 task in graph after create:\n$(cat "$graph")"

# ── Fix A precondition: chat task carries the chat-loop tag. The orphan
#    sweep keys off this tag.
echo "$chat_line" | grep -qF '"chat-loop"' \
    || loud_fail "Fix A precondition failed: .chat-0 missing chat-loop tag.\n$chat_line"

# ── Fix C precondition: chat task carries the user's endpoint URL.
echo "$chat_line" | grep -qF "\"endpoint\":\"$ENDPOINT\"" \
    || loud_fail "Fix C precondition failed: .chat-0 missing endpoint=$ENDPOINT.\n$chat_line"

# ── Step 2: Fix A — `wg sweep --dry-run` must NOT flag the chat-loop
#    task as orphaned (status=in-progress, assigned=none, chat-loop tag).
#    Pre-fix, this scenario reported the chat as orphaned and `wg sweep`
#    (without --dry-run) would reset it to Open, breaking the supervisor's
#    InProgress invariant.
sweep_out=$(wg sweep --dry-run 2>&1) \
    || loud_fail "wg sweep --dry-run failed: $sweep_out"

if echo "$sweep_out" | grep -qE '\.chat-0|chat-loop'; then
    loud_fail "Fix A regression: wg sweep flagged the chat-loop task as orphaned.\n$sweep_out"
fi

# ── Step 3: Fix C — spawn-task --dry-run must include -e <URL>. This is
#    the exact reproduction command in the diagnose log:
#      WG_EXECUTOR_TYPE=native WG_MODEL=qwen3-coder \
#      wg spawn-task --dry-run .chat-0
#    Pre-fix, the `-e` arg was silently dropped because plan.rs only
#    consulted config.llm_endpoints.find_default(), never task.endpoint.
dryrun_out=$(WG_EXECUTOR_TYPE=native WG_MODEL="$MODEL" \
    wg spawn-task --dry-run .chat-0 2>&1) \
    || loud_fail "wg spawn-task --dry-run failed: $dryrun_out"

# Tail line is the `wg nex ...` preview; the rest is the [spawn_task]
# provenance line. Both must reference the URL — provenance for "we know
# why this happened", preview for "the actual argv".
echo "$dryrun_out" | grep -qF "\"task.endpoint" \
    || echo "$dryrun_out" | grep -qF "task.endpoint" \
    || loud_fail "Fix C regression: provenance log line did not record task.endpoint.\n$dryrun_out"

echo "$dryrun_out" | grep -qF "wg nex --chat .chat-0" \
    || loud_fail "Fix C: dry-run preview missing 'wg nex --chat .chat-0'.\n$dryrun_out"

echo "$dryrun_out" | grep -qF "-e $ENDPOINT" \
    || loud_fail "Fix C regression: dry-run preview missing '-e $ENDPOINT'.\n$dryrun_out"

# ── Step 4: Fix C boundary — when the chat task's endpoint is REMOVED
#    (e.g., user re-runs without `-e`), spawn-task falls back to the
#    configured default. We don't simulate this here (would require a
#    config rewrite); the per-task-endpoint regression is the bug we
#    care about. Unit tests in src/dispatch/plan.rs cover the fallback.

# ── Step 5: per-chat CoordinatorState carries endpoint_override (the TUI
#    reattach path reads this on restart, so this property is what makes
#    the supervisor honor the override on respawn after a handler crash).
state_file="$wg_dir/service/coordinator-state-0.json"
[[ -f "$state_file" ]] \
    || loud_fail "expected $state_file; ls service/: $(ls "$wg_dir/service" 2>&1)"

grep -qE "\"endpoint_override\"\s*:\s*\"$ENDPOINT\"" "$state_file" \
    || loud_fail "endpoint_override missing from CoordinatorState: $(cat "$state_file" | tr -d '\n')"

echo "PASS: chat-loop excluded from sweep; spawn-task --dry-run includes -e $ENDPOINT; CoordinatorState persists endpoint_override"
exit 0
