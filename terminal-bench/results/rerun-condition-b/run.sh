#!/usr/bin/env bash
# Re-run Condition B with proper stigmergic context
# Uses ConditionCAgent (same wg tools as B, but with skill injection + planning phase)
# This is the "corrected" B run — the original B gave tools without context.
#
# Parameters match original B run:
#   - Model: openrouter/minimax/minimax-m2.7
#   - Trials: 3 per task (89 × 3 = 267)
#   - Concurrency: 4
#   - Dataset: terminal-bench/terminal-bench-2
#   - --no-delete to preserve Docker image cache
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/../.."  # repo root = /home/erik/workgraph

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Terminal Bench: Re-run Condition B (with skill injection)  ║"
echo "║  Agent: ConditionCAgent (wg tools + skill prompt)           ║"
echo "║  Model: openrouter/minimax/minimax-m2.7                     ║"
echo "║  Trials: 89 × 3 = 267                                      ║"
echo "║  Concurrency: 4                                             ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""

# Pre-flight checks
if [ -z "${OPENROUTER_API_KEY:-}" ]; then
    echo "ERROR: OPENROUTER_API_KEY not set"
    exit 1
fi

# Check Docker image cache
echo "Checking Docker image cache..."
MISSING=$(bash terminal-bench/pre-pull-images.sh --check 2>&1 | grep "Need to pull:" | awk '{print $NF}')
if [ "$MISSING" -gt 5 ]; then
    echo "WARNING: $MISSING images missing. Run pre-pull-images.sh first."
    echo "Continuing anyway — affected tasks will fail with RuntimeError."
fi
echo ""

# Record start time
echo $$ > terminal-bench/results/rerun-condition-b/run.pid
date -Iseconds > terminal-bench/results/rerun-condition-b/started_at.txt

# Run Harbor
echo "Starting Harbor run at $(date -Iseconds)..."
harbor run \
    -d terminal-bench/terminal-bench-2 \
    --agent-import-path "wg.adapter:ConditionCAgent" \
    -m "openrouter/minimax/minimax-m2.7" \
    -k 3 \
    -n 4 \
    --job-name rerun-condition-b \
    --jobs-dir terminal-bench/results/rerun-condition-b \
    --no-delete \
    --debug \
    --ak "max_turns=50" \
    --ak "temperature=0.0" \
    -y \
    2>&1 | tee terminal-bench/results/rerun-condition-b/run.log

echo ""
echo "Run completed at $(date -Iseconds)"
date -Iseconds > terminal-bench/results/rerun-condition-b/finished_at.txt
