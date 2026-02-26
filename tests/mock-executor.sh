#!/bin/bash
# Mock executor for integration tests.
#
# Reads WG_TASK_ID and WG_FIXTURES_DIR from environment.
# Returns a canned response from a fixture file, or fails if no fixture exists.
# Also supports a "slow" fixture that sleeps before responding.

set -euo pipefail

TASK_ID="${WG_TASK_ID:-${TASK_ID:-unknown}}"
FIXTURES_DIR="${WG_FIXTURES_DIR:-tests/fixtures/executor}"
WG_DIR="${WG_DIR:-.workgraph}"

# Check for fixture file
FIXTURE_FILE="$FIXTURES_DIR/$TASK_ID.sh"
if [ -f "$FIXTURE_FILE" ]; then
    # Execute the fixture script
    bash "$FIXTURE_FILE"
    exit $?
fi

# Check for a simple response file
RESPONSE_FILE="$FIXTURES_DIR/$TASK_ID.response"
if [ -f "$RESPONSE_FILE" ]; then
    cat "$RESPONSE_FILE"
    exit 0
fi

# Default: mark the task as done via wg CLI
echo "Mock executor: completing task $TASK_ID"
wg --dir "$WG_DIR" done "$TASK_ID" 2>/dev/null || true
exit 0
