#!/usr/bin/env bash
# Smoke: keyring backend round-trip (set / get / list / rm).
# owner: implement-wg-secret
#
# exit 0  → PASS
# exit 77 → loud SKIP (keyring dir unavailable)
# any other non-zero → FAIL
set -euo pipefail
. "$(dirname "$0")/_helpers.sh"

# Use a unique name to avoid collisions with concurrent tests
SECRET_NAME="smoke-keyring-$$"
SECRET_VALUE="sk-smoke-test-value-${RANDOM}"

# Ensure we clean up even on failure
cleanup_secret() {
    wg secret rm "$SECRET_NAME" 2>/dev/null || true
}
add_cleanup_hook cleanup_secret

# ── set ──────────────────────────────────────────────────────────────────────
wg secret set "$SECRET_NAME" --value "$SECRET_VALUE" 2>&1 | grep -q "stored in keyring backend" \
    || { echo "FAIL: wg secret set did not report success"; exit 1; }

# ── get (redacted) ───────────────────────────────────────────────────────────
wg secret get "$SECRET_NAME" 2>&1 | grep -q "exists:" \
    || { echo "FAIL: wg secret get did not show key exists"; exit 1; }

# get --reveal must show the actual value
REVEALED=$(wg secret get "$SECRET_NAME" --reveal 2>/dev/null)
[ "$REVEALED" = "$SECRET_VALUE" ] \
    || { echo "FAIL: --reveal returned '$REVEALED', expected '$SECRET_VALUE'"; exit 1; }

# ── list ─────────────────────────────────────────────────────────────────────
wg secret list 2>&1 | grep -q "keyring:${SECRET_NAME}" \
    || { echo "FAIL: wg secret list did not include the secret name"; exit 1; }

# ── check ref ────────────────────────────────────────────────────────────────
wg secret check "keyring:${SECRET_NAME}" 2>&1 | grep -q "is reachable" \
    || { echo "FAIL: wg secret check did not report reachable"; exit 1; }

# ── rm ───────────────────────────────────────────────────────────────────────
wg secret rm "$SECRET_NAME" 2>&1 | grep -q "deleted from keyring backend" \
    || { echo "FAIL: wg secret rm did not report success"; exit 1; }

# After rm, list should no longer include it
wg secret list 2>&1 | grep -qv "keyring:${SECRET_NAME}" \
    || { echo "FAIL: wg secret list still shows the deleted secret"; exit 1; }

# After rm, check should report not reachable
wg secret check "keyring:${SECRET_NAME}" 2>&1 | grep -q "NOT reachable" \
    || { echo "FAIL: wg secret check should report not reachable after rm"; exit 1; }

echo "PASS: keyring backend round-trip OK"
exit 0
