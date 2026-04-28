#!/usr/bin/env bash
# Smoke: wg profile diff shows differences between profiles
set -eu
source "$(dirname "$0")/_helpers.sh"
require_wg

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME/.wg"

wg profile init-starters 2>&1

# Diff claude vs codex — should contain both model strings
diff_out=$(wg profile diff claude codex 2>&1)
if ! echo "$diff_out" | grep -q "claude:opus"; then
    loud_fail "wg profile diff should show 'claude:opus': $diff_out"
fi
if ! echo "$diff_out" | grep -q "codex:gpt-5.5"; then
    loud_fail "wg profile diff should show 'codex:gpt-5.5': $diff_out"
fi

echo "PASS: profile_diff"
