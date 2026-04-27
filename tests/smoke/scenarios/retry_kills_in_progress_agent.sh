#!/usr/bin/env bash
# Scenario: retry_kills_in_progress_agent
#
# Pins add-wg-retry: `wg retry <task-id>` and `wg agents kill <agent-id>`
# must terminate a hung worker, mark the agent Dead in the registry, and
# reset the task to Open with retry_count incremented so the dispatcher
# respawns a fresh agent on its next tick.
#
# Without these primitives, users had to hunt for the PID via
# `wg agents --alive`, kill it manually, and wait ~30s for the reaper.
#
# Strategy:
#   1. Init a workgraph project.
#   2. Spawn a long-running `sleep 600` to stand in for a hung worker.
#   3. Hand-craft graph.jsonl: one InProgress task assigned to "agent-99"
#      that points at the sleep PID.
#   4. Run `wg agents kill agent-99` — assert sleep process exits and
#      the registry marks the agent Dead.
#   5. Re-run `wg agents kill agent-99` — assert it's a no-op (no error).
#   6. Spawn a second sleep, point another in-progress task at it.
#   7. Run `wg retry <task>` — assert sleep killed, task Open,
#      retry_count incremented from 0 to 1.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch="$(make_scratch)"
cd "$scratch" || loud_fail "could not cd into scratch $scratch"

git init -q || loud_fail "git init failed"
git config user.email smoke@example.com
git config user.name "smoke"
echo "init" > README.md
git add README.md
git commit -qm "init" || loud_fail "git commit failed"

wg init -m local:test -e http://127.0.0.1:1 >init.log 2>&1 \
    || loud_fail "wg init failed: $(cat init.log)"

if [ -d .wg ]; then
    wg_dir=".wg"
elif [ -d .workgraph ]; then
    wg_dir=".workgraph"
else
    loud_fail "could not find workgraph dir after init"
fi

graph="$wg_dir/graph.jsonl"
mkdir -p "$wg_dir/service"

# ── Test 1: wg agents kill on a live PID ─────────────────────────────
sleep 600 &
sleep_pid_a=$!
disown $sleep_pid_a 2>/dev/null || true
trap '[ -n "${sleep_pid_a:-}" ] && kill -9 "$sleep_pid_a" 2>/dev/null; [ -n "${sleep_pid_b:-}" ] && kill -9 "$sleep_pid_b" 2>/dev/null' EXIT

# Confirm the sleep is actually alive before we proceed.
kill -0 "$sleep_pid_a" 2>/dev/null \
    || loud_fail "test setup: sleep process A ($sleep_pid_a) not alive"

now="$(date -u +%Y-%m-%dT%H:%M:%S.%6NZ)"

# Hand-craft graph.jsonl: one InProgress task assigned to agent-99.
cat > "$graph" <<EOF
{"kind":"task","id":"hung-task-a","title":"Hung task A","status":"in-progress","assigned":"agent-99","priority":10,"created_at":"$now"}
EOF

cat > "$wg_dir/service/registry.json" <<EOF
{
  "agents": {
    "agent-99": {
      "id": "agent-99",
      "pid": $sleep_pid_a,
      "task_id": "hung-task-a",
      "executor": "claude",
      "started_at": "$now",
      "last_heartbeat": "$now",
      "status": "working",
      "output_file": "/tmp/a.log"
    }
  },
  "next_agent_id": 100
}
EOF

# ── Run `wg agents kill` ──────────────────────────────────────────────
wg agents kill agent-99 >kill.log 2>&1 \
    || loud_fail "wg agents kill failed: $(cat kill.log)"

# Wait up to 6s for sleep to actually die (graceful + escalation window).
for _ in $(seq 1 30); do
    if ! kill -0 "$sleep_pid_a" 2>/dev/null; then
        break
    fi
    sleep 0.2
done

if kill -0 "$sleep_pid_a" 2>/dev/null; then
    echo "----- kill.log -----" 1>&2
    cat kill.log 1>&2
    loud_fail "REGRESSION: wg agents kill did not terminate sleep PID $sleep_pid_a"
fi

# Registry should mark agent-99 as Dead now.
status_a=$(grep -oE '"status":[[:space:]]*"[^"]+"' "$wg_dir/service/registry.json" | head -1)
case "$status_a" in
    *dead*) ;;
    *)
        echo "----- registry.json -----" 1>&2
        cat "$wg_dir/service/registry.json" 1>&2
        loud_fail "REGRESSION: agent-99 not marked dead after kill, status: $status_a"
        ;;
esac

# ── Test 2: re-running kill on a now-dead agent is a no-op ───────────
wg agents kill agent-99 >kill2.log 2>&1 \
    || loud_fail "wg agents kill (idempotent re-run) errored: $(cat kill2.log)"

# ── Test 3: wg agents kill on a missing agent is a no-op ─────────────
wg agents kill agent-doesnotexist >kill3.log 2>&1 \
    || loud_fail "wg agents kill (missing agent) errored: $(cat kill3.log)"

# ── Test 4: wg retry on an in-progress hung task ─────────────────────
sleep 600 &
sleep_pid_b=$!
disown $sleep_pid_b 2>/dev/null || true

kill -0 "$sleep_pid_b" 2>/dev/null \
    || loud_fail "test setup: sleep process B ($sleep_pid_b) not alive"

now2="$(date -u +%Y-%m-%dT%H:%M:%S.%6NZ)"

# Reset graph.jsonl with a fresh in-progress task assigned to agent-200.
cat > "$graph" <<EOF
{"kind":"task","id":"hung-task-b","title":"Hung task B","status":"in-progress","assigned":"agent-200","priority":10,"created_at":"$now2"}
EOF

cat > "$wg_dir/service/registry.json" <<EOF
{
  "agents": {
    "agent-200": {
      "id": "agent-200",
      "pid": $sleep_pid_b,
      "task_id": "hung-task-b",
      "executor": "claude",
      "started_at": "$now2",
      "last_heartbeat": "$now2",
      "status": "working",
      "output_file": "/tmp/b.log"
    }
  },
  "next_agent_id": 201
}
EOF

wg retry hung-task-b --reason "smoke test" >retry.log 2>&1 \
    || loud_fail "wg retry failed: $(cat retry.log)"

# Sleep should be killed.
for _ in $(seq 1 30); do
    if ! kill -0 "$sleep_pid_b" 2>/dev/null; then
        break
    fi
    sleep 0.2
done
if kill -0 "$sleep_pid_b" 2>/dev/null; then
    echo "----- retry.log -----" 1>&2
    cat retry.log 1>&2
    loud_fail "REGRESSION: wg retry did not terminate hung agent PID $sleep_pid_b"
fi

# Task should be Open now with retry_count incremented.
status_b=$(wg show hung-task-b 2>/dev/null | grep -E "^Status:" | head -1)
case "$status_b" in
    *open*|*Open*) ;;
    *)
        echo "----- wg show hung-task-b -----" 1>&2
        wg show hung-task-b 1>&2
        loud_fail "REGRESSION: task not Open after wg retry, status: $status_b"
        ;;
esac

# Check graph.jsonl directly for retry_count == 1.
rc=$(grep -oE '"retry_count":[[:space:]]*[0-9]+' "$graph" | head -1 | grep -oE '[0-9]+$')
if [ "$rc" != "1" ]; then
    echo "----- graph.jsonl -----" 1>&2
    cat "$graph" 1>&2
    loud_fail "REGRESSION: retry_count expected 1, got '$rc' after wg retry on in-progress"
fi

# Reason should appear in the task log.
if ! grep -q "smoke test" "$graph"; then
    echo "----- graph.jsonl -----" 1>&2
    cat "$graph" 1>&2
    loud_fail "REGRESSION: --reason 'smoke test' not recorded in task log"
fi

echo "OK: wg agents kill kills hung agents idempotently; wg retry kills + resets in-progress tasks"
exit 0
