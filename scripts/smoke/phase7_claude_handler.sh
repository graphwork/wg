#!/usr/bin/env bash
# Phase 7 live smoke: `wg claude-handler` — the standalone Claude
# adapter that bridges Claude CLI ↔ chat/*.jsonl so spawn-task can
# dispatch to it the same way it dispatches to `wg nex`.
#
# Assertions (TDD — these are expected to FAIL until Phase 7 lands):
#   A. `wg claude-handler --help` succeeds (subcommand exists)
#   B. Handler acquires the session lock on its chat dir
#   C. Handler reads an inbox message and drives Claude CLI
#   D. Handler writes a response to outbox.jsonl
#   E. Handler releases the lock on clean shutdown
#
# Proves: the Claude path is a peer of the native path under spawn-task.
set -euo pipefail
tmp=$(mktemp -d)
trap "pkill -P $$ 2>/dev/null || true; pkill -f 'claude-handler' 2>/dev/null || true; rm -rf $tmp" EXIT
cd "$tmp"
export WG_DIR="$tmp/.workgraph"
wg init --no-agency >/dev/null 2>&1

fail() { echo "FAIL: $*"; exit 1; }
pass() { echo "PASS: $*"; }

echo "=== Test A: claude-handler subcommand exists ==="
if wg claude-handler --help >/dev/null 2>&1; then
  pass "wg claude-handler --help works"
else
  fail "wg claude-handler subcommand missing"
fi

echo
echo "=== Test B: handler acquires session lock ==="
chat_ref="smoke-claude"
chat_dir="$WG_DIR/chat/$chat_ref"
mkdir -p "$chat_dir"
# Run handler in background; it should acquire the lock immediately.
wg claude-handler --chat "$chat_ref" >"$tmp/handler.out" 2>"$tmp/handler.err" &
hp=$!
for i in {1..30}; do
  [ -f "$chat_dir/.handler.pid" ] && break
  sleep 0.3
done
if [ -f "$chat_dir/.handler.pid" ]; then
  kind=$(sed -n 3p "$chat_dir/.handler.pid" 2>/dev/null || echo "")
  pass "handler acquired lock (kind=$kind)"
else
  echo "DIAG: handler.err:"
  cat "$tmp/handler.err" 2>/dev/null | head -20
  fail "handler did not create .handler.pid within 9s"
fi

echo
echo "=== Test C/D: handler processes an inbox message ==="
# Write an inbox message that Claude should respond to. The parser
# requires id+timestamp+role+content+request_id (see `ChatMessage`
# in src/chat.rs). In production these come from `chat::append_inbox_ref`;
# here we format a valid row directly.
req_id="req-$(date +%s)"
ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
printf '{"id":1,"timestamp":"%s","role":"user","content":"Reply with just the word: smoke","request_id":"%s"}\n' "$ts" "$req_id" \
  >> "$chat_dir/inbox.jsonl"

# Wait up to 60s for outbox response
for i in {1..120}; do
  if [ -s "$chat_dir/outbox.jsonl" ]; then
    break
  fi
  sleep 0.5
done
if [ -s "$chat_dir/outbox.jsonl" ]; then
  pass "handler produced outbox response"
  echo "  outbox tail:"
  tail -1 "$chat_dir/outbox.jsonl" | head -c 200
  echo
else
  echo "DIAG: handler.err:"
  tail -30 "$tmp/handler.err" 2>/dev/null
  fail "no outbox response within 60s"
fi

echo
echo "=== Test E: handler releases lock on shutdown ==="
kill -TERM $hp 2>/dev/null || true
for i in {1..20}; do
  kill -0 $hp 2>/dev/null || break
  sleep 0.3
done
# Lock file may linger briefly (the handler isn't fully gone yet) — give it a beat.
sleep 1
if [ -f "$chat_dir/.handler.pid" ]; then
  # Lock may remain as a file, but the recorded PID must be dead.
  lock_pid=$(sed -n 1p "$chat_dir/.handler.pid")
  if kill -0 "$lock_pid" 2>/dev/null; then
    fail "handler lock still held by live PID $lock_pid after SIGTERM"
  fi
fi
pass "handler released lock (or lock owner is dead)"

echo
echo "=== ALL PHASE 7 CLAUDE HANDLER CHECKS PASSED ==="
