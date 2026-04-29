#!/usr/bin/env bash
# Scenario: chat_create_endpoint_persists
#
# Regression lock for fix-chat-creation: `wg chat create -e <URL>` (and the
# legacy `wg service create-coordinator --endpoint <URL>` alias the TUI
# launcher invokes) MUST persist the endpoint into the per-chat
# CoordinatorState file, AND record `endpoint` on the `.chat-N` graph task
# itself, so the supervisor honors the override on first spawn AND on
# respawn after handler crash / TUI restart.
#
# Pre-fix the launcher captured the endpoint into history but the resulting
# chat used the daemon-default endpoint — exactly the gap the user reported
# with `wg nex -m qwen3-coder -e https://lambda01...`.
#
# Pure graph-state correctness check; no LLM call required.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

if ! wg init -m claude:opus >init.log 2>&1; then
    loud_fail "wg init -m claude:opus failed: $(tail -5 init.log)"
fi

# Locate the workgraph dir (init writes it as `.wg/` by default).
wg_dir="$scratch/.wg"
if [[ ! -d "$wg_dir" ]]; then
    loud_fail "expected $wg_dir after wg init; got: $(ls -la "$scratch")"
fi

ENDPOINT='https://lambda01.tail334fe6.ts.net:30000'
MODEL='qwen3-coder'

# Service is down — exercises the run_create_direct path (the TUI launcher
# eventually goes via IPC, but persistence behaviour is identical because
# both share `create_chat_in_graph`).
out=$(wg chat create \
    --name lambdatest \
    --executor native \
    --model "$MODEL" \
    --endpoint "$ENDPOINT" \
    --json 2>&1) || loud_fail "wg chat create -e failed: $out"

# Graph task must carry endpoint AND model fields.
graph="$wg_dir/graph.jsonl"
if [[ ! -f "$graph" ]]; then
    loud_fail "graph.jsonl missing after create: $wg_dir"
fi

chat_line=$(grep -E '"id":"\.chat-0"' "$graph" | head -1)
if [[ -z "$chat_line" ]]; then
    loud_fail "no .chat-0 task in graph after create:\n$(cat "$graph")"
fi

if ! echo "$chat_line" | grep -qF "\"endpoint\":\"$ENDPOINT\""; then
    loud_fail ".chat-0 task is missing endpoint=$ENDPOINT.\nLine: $chat_line"
fi
if ! echo "$chat_line" | grep -qF "\"model\":\"$MODEL\""; then
    loud_fail ".chat-0 task is missing model=$MODEL.\nLine: $chat_line"
fi

# CoordinatorState file must persist all three overrides so a TUI restart
# (per implement-tmux-wrapped) can reattach with the original handler /
# model / endpoint.
state_file="$wg_dir/service/coordinator-state-0.json"
if [[ ! -f "$state_file" ]]; then
    loud_fail "expected per-chat coordinator-state at $state_file; ls service/: $(ls "$wg_dir/service" 2>&1)"
fi

# `serde_json::to_string_pretty` writes `"key": "value"` (with a space).
# Match the value half so we don't depend on prettifier whitespace.
state=$(cat "$state_file")
for pair in \
    "executor_override:native" \
    "model_override:$MODEL" \
    "endpoint_override:$ENDPOINT"; do
    key="${pair%%:*}"
    val="${pair#*:}"
    if ! grep -E "\"$key\"\s*:\s*\"$val\"" "$state_file" >/dev/null 2>&1; then
        loud_fail "CoordinatorState missing $key=$val. File contents: $(cat "$state_file" | tr -d '\n')"
    fi
done

# Equivalence check: `wg service create-coordinator` (the alias the TUI
# launcher actually invokes) MUST also accept --endpoint and produce the
# same persistence shape.
out2=$(wg service create-coordinator \
    --name lambdatest2 \
    --executor native \
    --model "$MODEL" \
    --endpoint "$ENDPOINT" \
    --json 2>&1) || loud_fail "wg service create-coordinator -e failed: $out2"

state_file2="$wg_dir/service/coordinator-state-1.json"
if [[ ! -f "$state_file2" ]]; then
    loud_fail "expected coordinator-state-1 at $state_file2; got: $(ls "$wg_dir/service" 2>&1)"
fi
if ! grep -E "\"endpoint_override\"\s*:\s*\"$ENDPOINT\"" "$state_file2" >/dev/null 2>&1; then
    loud_fail "second chat (via service create-coordinator alias) lost endpoint override: $(cat "$state_file2" | tr -d '\n')"
fi

echo "PASS: chat-0 + chat-1 both persist endpoint=$ENDPOINT, model=$MODEL, executor=native"
exit 0
