#!/usr/bin/env bash
# Scenario: launcher_custom_url_register_persists
#
# Regression lock for fix-new-chat: the TUI new-chat dialog allows users
# to drop in an ad-hoc Custom URL inline AND optionally register it with
# a name. Filling the name in must:
#   1. Persist the endpoint to PROJECT config (`.wg/config.toml`) so it
#      shows up in `wg endpoint list` afterward (and in subsequent
#      launcher opens).
#   2. Still create the chat with that URL as the per-chat endpoint
#      override.
#
# This scenario exercises the underlying CLI primitives the launcher
# invokes — `wg endpoint add` (project-local) followed by
# `wg service create-coordinator --endpoint <URL>`. The TUI cannot be
# driven from a smoke script directly, but the launcher's
# `launch_from_launcher` path lowers to exactly these two operations,
# so a passing smoke here gives us regression coverage for the spec
# the user reported.
#
# Project-local (not global) registration: the launcher's
# `open_launcher` reads through `Config::load_or_default` which strips
# global endpoints unless `[llm_endpoints] inherit_global = true` is
# set. Saving globally here would silently fail the user's "register
# so it shows up next time" expectation.
#
# exit 0  → PASS
# exit 77 → loud SKIP (no preconditions to check — pure offline test)
# any other non-zero → FAIL

set -euo pipefail
. "$(dirname "$0")/_helpers.sh"
require_wg

# Isolate HOME so any background config reads can't pollute global config.
SMOKE_HOME=$(mktemp -d)
add_cleanup_hook "rm -rf $SMOKE_HOME"
export HOME="$SMOKE_HOME"
mkdir -p "$SMOKE_HOME/.wg"

scratch=$(make_scratch)
cd "$scratch"

if ! wg init -m claude:opus >init.log 2>&1; then
    echo "FAIL: wg init: $(tail -5 init.log)"
    exit 1
fi

URL='https://my-lab.example.com:8080'
NAME='ad-hoc-lab'
MODEL='qwen3-coder'

# Step 1: launcher would call this when register_endpoint_name is filled.
# Project-local (no --global), matching launch_from_launcher.
if ! wg endpoint add --url "$URL" "$NAME" >add.log 2>&1; then
    echo "FAIL: wg endpoint add: $(cat add.log)"
    exit 1
fi

# The newly-registered endpoint must appear in `wg endpoint list`.
LIST_OUT=$(wg endpoint list 2>&1) || {
    echo "FAIL: wg endpoint list errored: $LIST_OUT"
    exit 1
}
if ! grep -qF "$NAME" <<<"$LIST_OUT"; then
    echo "FAIL: registered endpoint '$NAME' missing from list"
    echo "list output:"
    echo "$LIST_OUT"
    exit 1
fi
if ! grep -qF "$URL" <<<"$LIST_OUT"; then
    echo "FAIL: registered endpoint URL '$URL' missing from list"
    echo "list output:"
    echo "$LIST_OUT"
    exit 1
fi

# Step 2: launcher's chat-create call (one-shot URL still passed inline).
out=$(wg chat create \
    --name lab-test \
    --executor native \
    --model "$MODEL" \
    --endpoint "$URL" \
    --json 2>&1) || {
    echo "FAIL: wg chat create: $out"
    exit 1
}

# The chat task in the graph should pin the endpoint to the URL.
graph="$scratch/.wg/graph.jsonl"
if ! grep -E '"id":"\.chat-0"' "$graph" | grep -qF "\"endpoint\":\"$URL\""; then
    echo "FAIL: .chat-0 task missing endpoint=$URL"
    grep -E '"id":"\.chat-0"' "$graph" || echo "(no .chat-0 row)"
    exit 1
fi

# Per-chat coordinator state should also persist the URL so a TUI
# restart reattaches with the correct endpoint.
state_file="$scratch/.wg/service/coordinator-state-0.json"
if [[ ! -f "$state_file" ]]; then
    echo "FAIL: missing coordinator-state-0.json"
    ls "$scratch/.wg/service" 2>&1
    exit 1
fi
if ! grep -qF "\"endpoint_override\": \"$URL\"" "$state_file" \
   && ! grep -qF "\"endpoint_override\":\"$URL\"" "$state_file"; then
    echo "FAIL: endpoint_override missing/wrong in CoordinatorState"
    cat "$state_file"
    exit 1
fi

# Sanity: pre-registered endpoints must still appear AFTER chat creation.
LIST_AFTER=$(wg endpoint list 2>&1)
if ! grep -qF "$NAME" <<<"$LIST_AFTER"; then
    echo "FAIL: pre-registered '$NAME' disappeared after chat creation"
    echo "$LIST_AFTER"
    exit 1
fi

echo "PASS: registered endpoint persisted + chat pinned to URL"
exit 0
