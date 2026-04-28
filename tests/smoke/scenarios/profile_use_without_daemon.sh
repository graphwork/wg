#!/usr/bin/env bash
# Smoke: wg profile use (no daemon) — verify active-profile and config merge
set -eu
source "$(dirname "$0")/_helpers.sh"
require_wg

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME/.wg"

# Init starters
wg profile init-starters 2>&1

# Activate codex
wg profile use codex --no-reload 2>&1

# Assert: active-profile file contains "codex"
active=$(cat "$HOME/.wg/active-profile" 2>/dev/null || echo "")
if [[ "$active" != "codex" ]]; then
    loud_fail "~/.wg/active-profile should be 'codex', got: '$active'"
fi

# Assert: wg config --merged shows codex agent model
# (needs a workgraph dir to exist for wg config --merged)
mkdir -p "$scratch/proj/.workgraph"
merged=$(WG_DIR="$scratch/proj/.workgraph" wg --dir "$scratch/proj/.workgraph" config --show 2>&1)
if ! echo "$merged" | grep -q "codex:gpt-5.5"; then
    loud_fail "wg config --merged should reflect codex:gpt-5.5 after 'wg profile use codex': $merged"
fi

# Clear the profile
wg profile use --clear --no-reload 2>&1

# Assert: active-profile file removed
if [[ -f "$HOME/.wg/active-profile" ]]; then
    loud_fail "~/.wg/active-profile should be removed after 'wg profile use --clear'"
fi

echo "PASS: profile_use_without_daemon"
