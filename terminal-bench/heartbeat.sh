#!/usr/bin/env bash
# Phase 0 external heartbeat: sends periodic synthetic messages to the
# coordinator agent, triggering it to review graph state and take action.
#
# Usage:
#   ./heartbeat.sh [interval_secs] [coordinator_id]
#
# This is a fallback for environments without integrated heartbeat support
# (Option B from the design doc). For production use, configure
# coordinator.heartbeat_interval in config.toml instead (Option A).

set -euo pipefail

INTERVAL="${1:-30}"
COORDINATOR_ID="${2:-0}"
TICK=0

echo "Heartbeat loop starting: interval=${INTERVAL}s, coordinator=${COORDINATOR_ID}"

while true; do
    TICK=$((TICK + 1))
    TIMESTAMP=$(date +%H:%M:%S)

    MSG="[AUTONOMOUS HEARTBEAT] Tick #${TICK} at ${TIMESTAMP}

You are the autonomous coordinator. No human operator.
Review the system state and take action:

1. STUCK AGENTS: Any agent running >5min with no output? → wg kill <id> and retry
2. FAILED TASKS: Any tasks failed? → Analyze cause, create fix-up task or retry
3. READY WORK: Unblocked tasks waiting? → Ensure they'll be dispatched
4. PROGRESS CHECK: Is the work converging toward completion?
5. STRATEGIC: Should any running approach be abandoned?

If everything is nominal, respond: NOOP — all systems nominal.
If you take action, log what and why."

    if wg msg send ".coordinator-${COORDINATOR_ID}" "${MSG}" 2>/dev/null; then
        echo "[${TIMESTAMP}] Heartbeat #${TICK} sent"
    else
        echo "[${TIMESTAMP}] Heartbeat #${TICK} failed (service not running?)"
    fi

    sleep "${INTERVAL}"
done
