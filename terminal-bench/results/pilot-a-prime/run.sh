#!/usr/bin/env bash
# Pilot Condition A' — bare agent, no wg, no turn cap, 30-min timeout
#
# Differences from original Condition A:
#   - No turn limit (max_turns=9999 — effectively unlimited)
#   - 30-minute agent timeout (2× default 900s)
#   - Only 10 pilot tasks (from pilot-tasks.json)
#   - 3 trials per task (30 total)
#   - Improved structured logging (via tb_logging.py TrialLogger)
#
# Parameters:
#   Model:       openrouter/minimax/minimax-m2.7
#   Agent:       wg.adapter:ConditionAAgent
#   Trials:      3 per task (30 total)
#   Concurrency: 4
#   Timeout:     30 minutes per trial (agent-timeout-multiplier=2.0)
#   Turn cap:    None (max_turns=9999)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RESULTS_DIR="$SCRIPT_DIR"
cd "$SCRIPT_DIR/../../.."  # repo root = /home/erik/workgraph

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Terminal Bench: Pilot Condition A' (no turn cap, 30m)      ║"
echo "║  Agent: ConditionAAgent (bare, no wg tools)                 ║"
echo "║  Model: openrouter/minimax/minimax-m2.7                     ║"
echo "║  Tasks: 10 pilot tasks × 3 trials = 30 total               ║"
echo "║  Concurrency: 4                                             ║"
echo "║  Timeout: 30 minutes | Turn cap: none (9999)                ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""

# Pre-flight checks
if [ -z "${OPENROUTER_API_KEY:-}" ]; then
    echo "ERROR: OPENROUTER_API_KEY not set"
    exit 1
fi

# Check Docker
if ! docker info &>/dev/null; then
    echo "ERROR: Docker not running"
    exit 1
fi

# Pilot task IDs (from terminal-bench/analysis/pilot-tasks.json)
PILOT_TASKS=(
    build-cython-ext
    cancel-async-tasks
    nginx-request-logging
    overfull-hbox
    regex-log
    count-dataset-tokens
    custom-memory-heap-crash
    merge-diff-arc-agi-task
    qemu-startup
    sparql-university
)

# Build include flags (task names need terminal-bench/ prefix for the registry)
INCLUDE_FLAGS=""
for task in "${PILOT_TASKS[@]}"; do
    INCLUDE_FLAGS="$INCLUDE_FLAGS -i terminal-bench/$task"
done

# Record start time
echo $$ > "$RESULTS_DIR/run.pid"
date -Iseconds > "$RESULTS_DIR/started_at.txt"

echo "Starting Harbor run at $(date -Iseconds)..."
echo "Tasks: ${PILOT_TASKS[*]}"
echo ""

# Run Harbor
harbor run \
    -d terminal-bench/terminal-bench-2 \
    --agent-import-path "wg.adapter:ConditionAAgent" \
    -m "openrouter/minimax/minimax-m2.7" \
    -k 3 \
    -n 4 \
    --job-name pilot-a-prime \
    --jobs-dir "$RESULTS_DIR" \
    --no-delete \
    --debug \
    --ak "max_turns=9999" \
    --ak "temperature=0.0" \
    --agent-timeout-multiplier 2.0 \
    $INCLUDE_FLAGS \
    -y \
    2>&1 | tee "$RESULTS_DIR/run.log"

echo ""
echo "Run completed at $(date -Iseconds)"
date -Iseconds > "$RESULTS_DIR/finished_at.txt"
