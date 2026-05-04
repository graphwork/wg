#!/usr/bin/env bash
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

wg init -m claude:opus >/tmp/chat-last-interaction-init.log 2>&1 \
    || loud_fail "wg init failed: $(tail -5 /tmp/chat-last-interaction-init.log)"

wg chat create --name older --json >/tmp/chat-last-interaction-c0.json 2>&1 \
    || loud_fail "create older chat failed: $(cat /tmp/chat-last-interaction-c0.json)"
sleep 1
wg chat create --name newer --json >/tmp/chat-last-interaction-c1.json 2>&1 \
    || loud_fail "create newer chat failed: $(cat /tmp/chat-last-interaction-c1.json)"

before=$(wg show .chat-0 2>&1) || loud_fail "show before send failed: $before"
echo "$before" | grep -qF "Last interaction:" \
    || loud_fail "wg show missing Last interaction before send: $before"

sleep 1
wg chat send .chat-0 "smoke: bump older chat activity" >/tmp/chat-last-interaction-send.log 2>&1 \
    || loud_fail "chat send failed: $(cat /tmp/chat-last-interaction-send.log)"

after=$(wg show .chat-0 2>&1) || loud_fail "show after send failed: $after"
echo "$after" | grep -qF "Last interaction:" \
    || loud_fail "wg show missing Last interaction after send: $after"

top_chat=$(python3 - <<'PY'
import json
from pathlib import Path

rows = []
for line in Path(".wg/graph.jsonl").read_text().splitlines():
    if not line.strip():
        continue
    obj = json.loads(line)
    if obj.get("kind") != "task":
        continue
    if "chat-loop" not in obj.get("tags", []):
        continue
    rows.append((obj.get("last_interaction_at") or obj.get("created_at"), obj["id"]))

rows.sort(reverse=True)
print(rows[0][1] if rows else "")
PY
)

[[ "$top_chat" == ".chat-0" ]] \
    || loud_fail "recently messaged chat did not sort first by last_interaction_at; top=$top_chat graph=$(cat .wg/graph.jsonl)"

echo "PASS: chat send bumps last_interaction_at, wg show displays it, and activity sort puts the chat first"
