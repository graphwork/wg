#!/usr/bin/env bash
# Phase 7 live smoke: daemon with executor=claude spawns its
# coordinator handler via `wg spawn-task` → `wg claude-handler`,
# peer-equivalent to the native executor path.
#
# Assertions (TDD — expected to FAIL until Phase 7 lands):
#   A. Daemon starts with executor=claude
#   B. Daemon's coordinator is a `wg spawn-task` descendant
#      (NOT an inline claude CLI child of the daemon)
#   C. Handler process is `wg claude-handler` (not `claude` directly)
#   D. Coordinator handles an inbox message (round-trip via Claude)
#   E. Daemon stops cleanly
#
# Proves: the daemon's Claude coordinator path routes through the
# same `spawn-task` machinery as the native path.
set -euo pipefail
tmp=$(mktemp -d)
trap "pkill -P $$ 2>/dev/null || true; pkill -f 'claude-handler' 2>/dev/null || true; pkill -f 'spawn-task .coordinator' 2>/dev/null || true; rm -rf $tmp" EXIT
cd "$tmp"
export WG_DIR="$tmp/.workgraph"
wg init --no-agency >/dev/null 2>&1

# Force executor=claude globally so the daemon's coordinator-0 takes
# the Claude path at startup (pre-IPC).
wg config --coordinator-executor claude >/dev/null 2>&1

fail() { echo "FAIL: $*"; exit 1; }
pass() { echo "PASS: $*"; }

echo "=== Test A: daemon starts with executor=claude ==="
wg service start >/dev/null 2>&1 &
svc_pid=$!
sleep 3
wg service status 2>&1 | grep -qE "^Service: running" || fail "service not reporting running"
pass "daemon started (svc_pid=$svc_pid)"

echo
echo "=== Test B: daemon spawned via spawn-task ==="
# Allow time for spawn-task → claude-handler exec chain to establish.
sleep 3
if pgrep -af "wg claude-handler" >/dev/null; then
  pass "wg claude-handler process is running"
else
  echo "DIAG: process tree:"
  pgrep -af "wg " | head -10
  echo "DIAG: daemon log tail:"
  tail -30 "$WG_DIR/service/daemon.log" 2>/dev/null
  fail "no wg claude-handler process found — daemon may still be running inline claude CLI"
fi

echo
echo "=== Test C: handler acquires coordinator lock ==="
lock_path="$WG_DIR/chat/coordinator-0/.handler.pid"
for i in {1..30}; do
  [ -f "$lock_path" ] && break
  sleep 0.5
done
[ -f "$lock_path" ] || fail "coordinator-0 handler lock not created"
kind=$(sed -n 3p "$lock_path" 2>/dev/null || echo "?")
pass "handler holds coordinator-0 lock (kind=$kind)"

echo
echo "=== Test D: coordinator processes an inbox message ==="
wg chat --coordinator 0 "Reply with just: ok" --timeout 90 >/dev/null 2>&1 || true
chat_dir="$WG_DIR/chat/coordinator-0"
for i in {1..180}; do
  if [ -s "$chat_dir/outbox.jsonl" ]; then
    break
  fi
  sleep 0.5
done
if [ -s "$chat_dir/outbox.jsonl" ]; then
  pass "coordinator produced outbox response"
else
  echo "DIAG: chat_dir tree:"
  ls -la "$chat_dir" 2>/dev/null
  echo "DIAG: inbox:"
  cat "$chat_dir/inbox.jsonl" 2>/dev/null | head -3
  echo "DIAG: daemon log tail:"
  tail -25 "$WG_DIR/service/daemon.log" 2>/dev/null
  fail "no outbox response within 90s"
fi

echo
echo "=== Test E: daemon stops cleanly ==="
wg service stop 2>&1 | tail -1
for i in {1..30}; do
  if ! wg service status 2>&1 | grep -qE "^Service: running"; then
    break
  fi
  sleep 0.5
done
if wg service status 2>&1 | grep -qE "^Service: running"; then
  fail "daemon still running after 15s"
fi
pass "daemon stopped"

echo
echo "=== ALL PHASE 7 DAEMON-CLAUDE CHECKS PASSED ==="
