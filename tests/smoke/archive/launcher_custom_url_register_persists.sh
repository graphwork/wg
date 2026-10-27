#!/usr/bin/env bash
# Scenario: launcher_custom_url_register_persists — ARCHIVED (retired). The
# endpoint-pinned chat route it pins was removed by task `make-pi-the-2`
# (commit 9e52f9f7, `docs/design-retire-model-registry.md`): a live chat is
# Pi-only, the executor is derived from the exact route, and the model must be
# handler-first, so `wg chat create --executor native --model <bare>
# --endpoint <url>` is now refused with `WG-EXEC-ROUTE-REQUIRED`. The script and
# its grow-only `[[scenario]]` manifest entry are preserved for history; the
# scenario SKIPs loudly (exit 77) instead of FAILing, because the subject — a
# chat pinned to a registered ad-hoc endpoint — no longer exists. The half that
# still lives (project-local `wg endpoint add` → `wg endpoint list`) is exercised
# by the guarded body below and is no longer the scenario's subject.
# Original header follows.
#
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

# ── Retirement guard: the legacy endpoint chat route is gone ─────────────
# The subject is the launcher's "register the ad-hoc Custom URL, then create the
# chat against it with `--executor <x> --model <bare-model> --endpoint <url>`"
# flow. That route was removed by `make-pi-the-2` (commit 9e52f9f7): chat routes
# are `pi:`-only and the endpoint/executor chat inputs are DEPRECATED/UNSUPPORTED,
# so the legacy call is refused before any graph write. SKIP loudly; never FAIL a
# scenario whose subject was removed on purpose. Restoring the surface makes this
# body run unchanged again.
probe=$(make_scratch)
mkdir -p "$probe/home"
(
    cd "$probe" || exit 1
    env -u WG_DIR -u WG_PROJECT_ROOT -u WG_TASK_ID -u WG_AGENT_ID \
        HOME="$probe/home" \
        wg chat create --name retire-probe \
            --executor native \
            --model qwen3-coder \
            --endpoint 'https://retired.invalid/v1' \
            --json 2>&1 || true
) >"$probe/probe.log" 2>&1
if grep -q 'WG-EXEC-ROUTE-REQUIRED' "$probe/probe.log"; then
    loud_skip "LEGACY ENDPOINT CHAT ROUTE RETIRED" \
        'the `wg chat create --executor <handler> --model <bare-model> --endpoint <url>` route was removed by task make-pi-the-2 (commit 9e52f9f7, docs/design-retire-model-registry.md): live chats are Pi-only and the model must be handler-first, so the endpoint-pinned chat this scenario asserts no longer exists.'
fi

# Isolate HOME so any background config reads can't pollute global config.
SMOKE_HOME=$(mktemp -d)
add_cleanup_hook "rm -rf $SMOKE_HOME"
export HOME="$SMOKE_HOME"
mkdir -p "$SMOKE_HOME/.wg"

scratch=$(make_scratch)
cd "$scratch"

if ! wg init -m pi:test:fake >init.log 2>&1; then
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
