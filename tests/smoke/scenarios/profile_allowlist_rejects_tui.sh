#!/usr/bin/env bash
# Smoke: wg profile use rejects a profile file with unknown field [tui]
set -eu
source "$(dirname "$0")/_helpers.sh"
require_wg

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME/.wg/profiles"

# Write a bad profile file with a disallowed [tui] section
cat > "$HOME/.wg/profiles/badprof.toml" << 'EOF'
description = "bad profile"
[tui]
theme = "dark"
EOF

# wg profile use should exit non-zero
if wg profile use badprof --no-reload 2>&1; then
    loud_fail "wg profile use badprof should have failed (exit non-zero)"
fi

# The error message should mention 'tui' as the unknown field
err_out=$(wg profile use badprof --no-reload 2>&1 || true)
if ! echo "$err_out" | grep -qi "tui"; then
    loud_fail "error message should mention 'tui': $err_out"
fi

echo "PASS: profile_allowlist_rejects_tui"
