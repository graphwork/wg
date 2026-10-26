#!/usr/bin/env bash
# Smoke: `wg setup --route openrouter --yes` must leave the user with the
# exact WG-managed login action when auth cannot be completed inline, while
# clearly distinguishing WG-managed vs Pi-managed OpenRouter auth.
# owner: integrate-openrouter-login
set -euo pipefail
. "$(dirname "$0")/_helpers.sh"
require_wg

SMOKE_HOME=$(mktemp -d)
PROJECT_ROOT=$(mktemp -d)
add_cleanup_hook "rm -rf $SMOKE_HOME $PROJECT_ROOT"
export HOME="$SMOKE_HOME"
unset WG_GLOBAL_DIR WG_DIR ANTHROPIC_API_KEY OPENROUTER_API_KEY OPENAI_API_KEY 2>/dev/null || true

WG_DIR="$PROJECT_ROOT/.wg"
mkdir -p "$WG_DIR" "$HOME/.wg"
cat >"$WG_DIR/graph.jsonl" <<'EOF'
EOF

SETUP_OUT=$(wg --dir "$WG_DIR" setup --route openrouter --yes 2>&1)

echo "$SETUP_OUT" | grep -q "Next independent login step: wg login openrouter" \
    || loud_fail "setup output did not give the exact next WG login action:\n$SETUP_OUT"
echo "$SETUP_OUT" | grep -q "WG-managed auth" \
    || loud_fail "setup output did not explain WG-managed auth:\n$SETUP_OUT"
echo "$SETUP_OUT" | grep -q "Pi keeps its own provider login separately" \
    || loud_fail "setup output did not distinguish Pi-managed auth:\n$SETUP_OUT"
echo "$SETUP_OUT" | grep -q "wg login openrouter --check" \
    || loud_fail "setup output missing follow-up check step:\n$SETUP_OUT"
echo "$SETUP_OUT" | grep -q "wg model-scout --no-cache" \
    || loud_fail "setup output missing model-scout follow-up:\n$SETUP_OUT"
echo "$SETUP_OUT" | grep -q "wg profile pi" \
    || loud_fail "setup output missing pi profile follow-up:\n$SETUP_OUT"

CONFIG="$HOME/.wg/config.toml"
[[ -f "$CONFIG" ]] || loud_fail "expected global config at $CONFIG after setup"
grep -q 'provider = "openrouter"' "$CONFIG" \
    || loud_fail "global config does not contain the openrouter endpoint:\n$(cat "$CONFIG")"

echo "PASS: wg setup --route openrouter prints the exact login handoff and Pi distinction"
