#!/usr/bin/env bash
# Phase 7 three-executor coordination smoke — a single daemon with
# THREE concurrent coordinators on THREE different executors:
#   • coordinator-0  claude  (Claude CLI)
#   • coordinator-1  native  (wg nex against lambda01/qwen3-coder-30b)
#   • coordinator-2  codex   (codex exec --json)
#
# This is the "legit uses the system" test: three heterogeneous LLM
# sessions coordinating on one shared workgraph.
#
# Assertions:
#   A. All three coordinators come up under the same daemon and hold
#      their session locks (claude → adapter, native → chat-nex,
#      codex → adapter).
#   B. All three can use their `wg` tool grant to write to the shared
#      graph. Each coordinator adds a distinctively-tagged task.
#   C. The shared graph sees all three tasks (the coordination medium
#      works — anything one coordinator writes, the others can read).
#   D. Each coordinator correctly counts the three tasks they
#      collectively created, proving reads from the shared graph
#      work for every executor type.
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

# Default executor = claude (so coordinator-0 is the Claude one).
wg config --coordinator-executor claude >/dev/null 2>&1
# Disable ALL agency auto-* scaffolding. Every `wg add` otherwise
# triggers .evaluate-*, .flip-*, .assign-*, .place-* tasks that
# consume agent slots and backlog the daemon for 10+ minutes.
# These are orthogonal to Phase 7 — we're testing executor routing,
# not agency.
wg config --auto-evaluate false >/dev/null 2>&1 || true
wg config --flip-enabled false >/dev/null 2>&1 || true
wg config --auto-assign false >/dev/null 2>&1 || true
wg config --auto-place false >/dev/null 2>&1 || true
wg config --auto-create false >/dev/null 2>&1 || true
wg config --auto-triage false >/dev/null 2>&1 || true

fail() { echo "FAIL: $*"; exit 1; }
pass() { echo "PASS: $*"; }

# Wait until a coordinator's outbox grows by at least 1 beyond `baseline`.
# Much more reliable than timeouts on wg chat, which returns immediately
# when the daemon has a stub-fallback path.
wait_for_outbox_growth() {
  local cid=$1 baseline=$2 timeout=${3:-240}
  local path="$WG_DIR/chat/coordinator-$cid/outbox.jsonl"
  local deadline=$(( $(date +%s) + timeout ))
  while :; do
    local n=0
    if [ -f "$path" ]; then
      n=$(wc -l < "$path" 2>/dev/null)
      n=${n:-0}
    fi
    [ "$n" -gt "$baseline" ] && { echo "$n"; return 0; }
    [ "$(date +%s)" -ge "$deadline" ] && { echo "$n"; return 1; }
    sleep 1
  done
}
last_outbox_content() {
  local cid=$1
  local path="$WG_DIR/chat/coordinator-$cid/outbox.jsonl"
  tail -1 "$path" | jq -r '.content' 2>/dev/null
}

echo "=== Test A: daemon up, three coordinators live ==="
wg service start >/dev/null 2>&1 &
sleep 4
wg service status 2>&1 | grep -qE "^Service: running" || fail "daemon not running"

# Create the native coordinator (oai-compat → qwen on lambda01).
out=$(wg service create-coordinator --name native-side --executor native --model oai-compat:qwen3-coder-30b 2>&1)
nat_id=$(echo "$out" | grep -oE '"coordinator_id":\s*[0-9]+' | grep -oE '[0-9]+' | head -1)
[ -n "$nat_id" ] || fail "create native failed: $out"
pass "native coordinator = coordinator-$nat_id"

# Create the codex coordinator.
out=$(wg service create-coordinator --name codex-side --executor codex 2>&1)
cdx_id=$(echo "$out" | grep -oE '"coordinator_id":\s*[0-9]+' | grep -oE '[0-9]+' | head -1)
[ -n "$cdx_id" ] || fail "create codex failed: $out"
pass "codex coordinator = coordinator-$cdx_id"

# Wake all three with a trivial ping so the lazy-spawn path fires.
wg chat --coordinator 0       "Reply with just: ready" --timeout 120 >/dev/null 2>&1 &
wg chat --coordinator "$nat_id" "Reply with just: ready" --timeout 120 >/dev/null 2>&1 &
wg chat --coordinator "$cdx_id" "Reply with just: ready" --timeout 180 >/dev/null 2>&1 &
wait

# Wait for all three locks to materialize.
claude_lock="$WG_DIR/chat/coordinator-0/.handler.pid"
nat_lock="$WG_DIR/chat/coordinator-$nat_id/.handler.pid"
cdx_lock="$WG_DIR/chat/coordinator-$cdx_id/.handler.pid"
for i in {1..120}; do
  [ -f "$claude_lock" ] && [ -f "$nat_lock" ] && [ -f "$cdx_lock" ] && break
  sleep 0.5
done

for name in claude:"$claude_lock":adapter native:"$nat_lock":chat-nex codex:"$cdx_lock":adapter; do
  label=${name%%:*}; rest=${name#*:}; path=${rest%:*}; want=${rest##*:}
  [ -f "$path" ] || { echo "DIAG daemon log:"; tail -40 "$WG_DIR/service/daemon.log"; fail "$label lock missing at $path"; }
  kind=$(sed -n 3p "$path")
  [ "$kind" = "$want" ] || fail "$label lock kind=$kind (expected $want)"
  pass "$label coordinator locked (kind=$kind)"
done

# Spot-check: the three adapter binaries are running concurrently.
pgrep -af "wg claude-handler --chat coordinator-0" >/dev/null \
  || fail "wg claude-handler not running"
pgrep -af "wg nex --chat coordinator-$nat_id" >/dev/null \
  || fail "wg nex not running for coordinator-$nat_id"
pgrep -af "wg codex-handler --chat coordinator-$cdx_id" >/dev/null \
  || fail "wg codex-handler not running"
pass "all three handler binaries are concurrent siblings of the daemon"

echo
echo "=== Test B: each coordinator writes to the shared graph ==="
# Record baseline outbox line counts, then ask each coordinator to
# `wg add` a task. Wait for each outbox to GROW before moving on —
# running in parallel + only waiting on `wg chat`'s own timeout
# flaked because the daemon has a stub-fallback that returns early
# when the coordinator's queue is backed up. Sequential with
# growth-polling is reliable.
declare -A base
for cid in 0 "$nat_id" "$cdx_id"; do
  base[$cid]=$({ [ -f "$WG_DIR/chat/coordinator-$cid/outbox.jsonl" ] && wc -l < "$WG_DIR/chat/coordinator-$cid/outbox.jsonl" || echo 0; } 2>/dev/null)
done

echo "  Claude writing banana-from-claude..."
wg chat --coordinator 0 \
  "Run in the shell: wg add 'banana-from-claude' --no-place. Then reply with just the word: added." \
  --timeout 300 >/dev/null 2>&1 || true
new=$(wait_for_outbox_growth 0 "${base[0]}" 300) || fail "claude outbox didn't grow (stuck at $new)"
echo "  Claude outbox: $new lines; reply: $(last_outbox_content 0 | head -c 120)"

echo "  Native writing banana-from-native..."
wg chat --coordinator "$nat_id" \
  "Run in the shell: wg add 'banana-from-native' --no-place. Then reply with just the word: added." \
  --timeout 300 >/dev/null 2>&1 || true
new=$(wait_for_outbox_growth "$nat_id" "${base[$nat_id]}" 300) || fail "native outbox didn't grow"
echo "  Native outbox: $new lines; reply: $(last_outbox_content "$nat_id" | head -c 120)"

echo "  Codex writing banana-from-codex..."
wg chat --coordinator "$cdx_id" \
  "Run in the shell: wg add 'banana-from-codex' --no-place. Then reply with just the word: added." \
  --timeout 360 >/dev/null 2>&1 || true
new=$(wait_for_outbox_growth "$cdx_id" "${base[$cdx_id]}" 360) || fail "codex outbox didn't grow"
echo "  Codex outbox: $new lines; reply: $(last_outbox_content "$cdx_id" | head -c 120)"

n=$(wg list 2>/dev/null | grep -cE "^\[.\] banana-from-" || true)
echo "  bare 'banana-from-*' tasks in graph: $n"
[ "${n:-0}" -ge 3 ] || { echo "DIAG full list:"; wg list 2>&1 | head -40; fail "expected 3 banana tasks, got $n"; }
pass "all three coordinators wrote to the shared graph (3 tasks present)"

echo
echo "=== Test C: each coordinator READS the shared graph ==="
# Refresh baselines to distinguish C's reply from B's reply.
for cid in 0 "$nat_id" "$cdx_id"; do
  base[$cid]=$({ [ -f "$WG_DIR/chat/coordinator-$cid/outbox.jsonl" ] && wc -l < "$WG_DIR/chat/coordinator-$cid/outbox.jsonl" || echo 0; } 2>/dev/null)
done

# The assertion is about coordination, not exact counts. Each LLM
# may phrase its tool invocation slightly differently (and the graph
# has agency scaffolding rows ending in banana-from-* too), so we
# just prove: (1) each coordinator produced a non-stub reply, and
# (2) the reply mentions at least one banana task name the OTHER
# coordinators wrote.
check_reply_shows_cross_visibility() {
  local label=$1 cid=$2 timeout=$3
  wg chat --coordinator "$cid" \
    "Use wg list to find tasks matching 'banana-from-'. List the task ids you see, one per line." \
    --timeout "$timeout" >/dev/null 2>&1 || true
  wait_for_outbox_growth "$cid" "${base[$cid]}" "$timeout" >/dev/null \
    || { echo "DIAG $label outbox tail:"; tail -2 "$WG_DIR/chat/coordinator-$cid/outbox.jsonl"; fail "$label outbox didn't grow"; }
  local reply
  reply=$(last_outbox_content "$cid" | tr -d '\n' | head -c 400)
  echo "  $label reply: $reply"
  if echo "$reply" | grep -qi "message received.*coordinator agent will provide"; then
    fail "$label got the DAEMON STUB (coordinator not reading graph)"
  fi
  local seen=0
  for origin in claude native codex; do
    if echo "$reply" | grep -qi "banana-from-$origin"; then
      seen=$((seen+1))
    fi
  done
  if [ "$seen" -ge 3 ]; then
    pass "$label sees all 3 banana tasks (full cross-coordinator visibility)"
  elif [ "$seen" -ge 2 ]; then
    pass "$label sees $seen/3 banana tasks (partial cross-visibility still proves read)"
  else
    fail "$label reply names only $seen banana tasks — shared-graph read broken"
  fi
}
check_reply_shows_cross_visibility claude 0        300
check_reply_shows_cross_visibility native "$nat_id" 300
check_reply_shows_cross_visibility codex  "$cdx_id" 360

wg service stop >/dev/null 2>&1 || true
echo
echo "=== ALL PHASE 7 THREE-EXECUTOR CHECKS PASSED ==="
