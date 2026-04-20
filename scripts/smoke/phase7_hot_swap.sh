#!/usr/bin/env bash
# Phase 7 hot-swap smoke — `wg service set-executor` flips a live
# coordinator's executor mid-conversation and the next turn goes
# through the new executor.
#
# Assertions:
#   A. Daemon up with executor=claude, coordinator-0 is a Claude
#      handler, first turn completes
#   B. `wg service set-executor 0 --executor native --model
#      oai-compat:qwen3-coder-30b` succeeds
#   C. Within 30s the handler has been replaced by a native `wg nex`
#      (session-lock kind transitions adapter → chat-nex)
#   D. The coordinator still responds to new messages after the swap
#      (conversation continuity via the shared chat/<ref>/*.jsonl)
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

wg service start >/dev/null 2>&1 &
sleep 4
wg service status 2>&1 | grep -qE "^Service: running" || fail "daemon not running"

echo "=== Test A: first turn on Claude ==="
wg chat --coordinator 0 "Reply: claude" --timeout 90 >/dev/null 2>&1 || true
for i in {1..120}; do
  [ -s "$WG_DIR/chat/coordinator-0/outbox.jsonl" ] && break
  sleep 1
done
[ -s "$WG_DIR/chat/coordinator-0/outbox.jsonl" ] || fail "no initial outbox"
lock_kind=$(sed -n 3p "$WG_DIR/chat/coordinator-0/.handler.pid")
[ "$lock_kind" = "adapter" ] || fail "expected claude (adapter) lock, got $lock_kind"
pass "Claude handler served first turn (lock=$lock_kind)"

echo
echo "=== Test B: hot-swap to native executor ==="
wg service set-executor 0 --executor native --model oai-compat:qwen3-coder-30b 2>&1 | head -5
pass "set-executor IPC accepted"

echo
echo "=== Test C: handler transitions to native wg nex ==="
for i in {1..60}; do
  kind=$(sed -n 3p "$WG_DIR/chat/coordinator-0/.handler.pid" 2>/dev/null || echo "")
  [ "$kind" = "chat-nex" ] && break
  sleep 0.5
done
kind=$(sed -n 3p "$WG_DIR/chat/coordinator-0/.handler.pid")
[ "$kind" = "chat-nex" ] || { echo "DIAG daemon log:"; tail -20 "$WG_DIR/service/daemon.log"; fail "handler kind stayed $kind — swap didn't take"; }
pass "handler is now native (lock=chat-nex)"

echo
echo "=== Test D: coordinator responds post-swap ==="
lines_before=$(wc -l < "$WG_DIR/chat/coordinator-0/outbox.jsonl")
wg chat --coordinator 0 "Reply: native" --timeout 90 >/dev/null 2>&1 || true
for i in {1..180}; do
  n=$(wc -l < "$WG_DIR/chat/coordinator-0/outbox.jsonl")
  [ "$n" -gt "$lines_before" ] && break
  sleep 0.5
done
n=$(wc -l < "$WG_DIR/chat/coordinator-0/outbox.jsonl")
[ "$n" -gt "$lines_before" ] || fail "no new outbox after swap ($lines_before == $n)"
pass "post-swap outbox grew ($lines_before → $n) — continuity preserved"

wg service stop >/dev/null 2>&1 || true
echo
echo "=== ALL PHASE 7 HOT-SWAP CHECKS PASSED ==="
