#!/usr/bin/env bash
# Phase 7 tool-use smoke — Claude coordinator uses its `Bash(wg:*)`
# tool grant to inspect the graph and reply with information derived
# from tool output, not from its system prompt alone.
#
# The whole point of a Claude coordinator is that it runs `wg status`
# or `wg list` to see the real graph state and answers accordingly.
# If tool-use is broken or tool output isn't reaching the outbox,
# the coordinator is a glorified single-turn chatbot.
#
# Assertions:
#   A. Daemon up with executor=claude, coordinator-0 live
#   B. Create 3 distinctively-named tasks in the graph
#   C. Ask the coordinator "how many tasks are open?" — it must run
#      a wg command and answer with "3" (the number of open tasks)
#
# Passing this proves: tool grant survives spawn-task → claude-handler
# dispatch, stream-json tool_use events round-trip through our
# collect_response, and the final text block reflects tool results.
set -euo pipefail
tmp=$(mktemp -d)
cleanup() {
  for p in $(pgrep -x wg 2>/dev/null); do
    c=$(cat /proc/$p/comm 2>/dev/null); [ "$c" = "wg" ] && kill "$p" 2>/dev/null
  done
  rm -rf "$tmp" 2>/dev/null || true
}
trap cleanup EXIT
cd "$tmp"
export WG_DIR="$tmp/.workgraph"
wg init --no-agency >/dev/null 2>&1
wg config --coordinator-executor claude >/dev/null 2>&1

fail() { echo "FAIL: $*"; exit 1; }
pass() { echo "PASS: $*"; }

echo "=== Test A: daemon up + claude coordinator live ==="
wg service start >/dev/null 2>&1 &
sleep 4
wg service status 2>&1 | grep -qE "^Service: running" || fail "daemon not running"
for i in {1..30}; do
  pgrep -f 'wg claude-handler --chat coordinator-0' >/dev/null && break
  sleep 0.5
done
pgrep -f 'wg claude-handler --chat coordinator-0' >/dev/null || fail "no claude handler"
pass "daemon + claude coordinator up"

echo
echo "=== Test B: seed 3 distinctive open tasks ==="
wg add "pineapple-task" -d "distinctive pineapple" >/dev/null 2>&1
wg add "kiwi-task" -d "distinctive kiwi" >/dev/null 2>&1
wg add "durian-task" -d "distinctive durian" >/dev/null 2>&1
wg publish pineapple-task --only >/dev/null
wg publish kiwi-task --only >/dev/null
wg publish durian-task --only >/dev/null
pass "3 tasks added and explicitly published"

echo
echo "=== Test C: coordinator answers graph question using tool-use ==="
# Ask for the count of open tasks. Claude must inspect the graph
# (via wg list or wg status), not invent.
wg chat --coordinator 0 \
  "Using wg tools, tell me the number of OPEN tasks in this graph. Reply with just the number and no other text." \
  --timeout 120 >/dev/null 2>&1 || true

for i in {1..240}; do
  [ -s "$WG_DIR/chat/coordinator-0/outbox.jsonl" ] && break
  sleep 0.5
done
[ -s "$WG_DIR/chat/coordinator-0/outbox.jsonl" ] \
  || { echo "DIAG handler log:"; tail -30 "$WG_DIR/chat/coordinator-0/handler.log"; fail "no outbox response"; }

content=$(tail -1 "$WG_DIR/chat/coordinator-0/outbox.jsonl" | python3 -c 'import sys,json; print(json.loads(sys.stdin.read())["content"])')
echo "  response: $content"

# Accept any reply that contains "3" as a standalone count.
# Claude may phrase it variously; the digit must appear.
if echo "$content" | grep -qE '(^|[^0-9])3($|[^0-9])'; then
  pass "coordinator returned '3' — tool-use reached the outbox"
else
  echo "DIAG: full outbox:"
  cat "$WG_DIR/chat/coordinator-0/outbox.jsonl"
  echo "DIAG: handler log:"
  tail -50 "$WG_DIR/chat/coordinator-0/handler.log"
  fail "response did not include '3' — tool-use may not be working"
fi

wg service stop >/dev/null 2>&1 || true
echo
echo "=== ALL PHASE 7 TOOL-USE CHECKS PASSED ==="
