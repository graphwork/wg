#!/usr/bin/env bash
# Smoke: profile create, list, and show (no daemon required)
set -eu
source "$(dirname "$0")/_helpers.sh"
require_wg

scratch=$(make_scratch)

# Use temp HOME so profiles go to isolated directory
export HOME="$scratch/home"
mkdir -p "$HOME/.wg"

# Create a profile
wg profile create test1 -m claude:opus --description "test profile" 2>&1

# Assert: profile file exists
if [[ ! -f "$HOME/.wg/profiles/test1.toml" ]]; then
    loud_fail "profile file not created at $HOME/.wg/profiles/test1.toml"
fi

# Assert: wg profile list contains test1 and marks it [user]
list_out=$(wg profile list 2>&1)
if ! echo "$list_out" | grep -q "test1"; then
    loud_fail "wg profile list does not contain test1: $list_out"
fi
if ! echo "$list_out" | grep -q "\[user\]"; then
    loud_fail "wg profile list does not mark as [user]: $list_out"
fi

# Assert: wg profile show test1 prints agent.model = "claude:opus"
show_out=$(wg profile show test1 2>&1)
if ! echo "$show_out" | grep -q 'claude:opus'; then
    loud_fail "wg profile show test1 does not show agent.model=claude:opus: $show_out"
fi

echo "PASS: profile_create_and_list"
