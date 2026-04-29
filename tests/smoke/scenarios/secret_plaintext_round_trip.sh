#!/usr/bin/env bash
# Smoke: plaintext backend round-trip (set / get / list / rm).
# Requires allow_plaintext = true in the test config.
# owner: implement-wg-secret
#
# exit 0  → PASS
# exit 77 → loud SKIP
# any other non-zero → FAIL
set -euo pipefail
. "$(dirname "$0")/_helpers.sh"

# We need a temp HOME with allow_plaintext enabled
SMOKE_HOME=$(mktemp -d)
add_cleanup_hook "rm -rf $SMOKE_HOME"
export HOME="$SMOKE_HOME"

# Write a minimal global config with allow_plaintext = true
mkdir -p "$SMOKE_HOME/.wg"
cat > "$SMOKE_HOME/.wg/config.toml" <<'TOML'
[secrets]
allow_plaintext = true
TOML

SECRET_NAME="smoke-plain-$$"
SECRET_VALUE="sk-plain-test-${RANDOM}"

cleanup_secret() {
    HOME="$SMOKE_HOME" wg secret rm "$SECRET_NAME" --backend plaintext 2>/dev/null || true
}
add_cleanup_hook cleanup_secret

# ── set ──────────────────────────────────────────────────────────────────────
HOME="$SMOKE_HOME" wg secret set "$SECRET_NAME" --backend plaintext --value "$SECRET_VALUE" 2>&1 \
    | grep -q "stored in plaintext backend" \
    || { echo "FAIL: wg secret set plaintext did not report success"; exit 1; }

# Verify file was created with 0600 perms
SECRET_FILE="$SMOKE_HOME/.wg/secrets/$SECRET_NAME"
if [ -f "$SECRET_FILE" ]; then
    PERMS=$(stat -c "%a" "$SECRET_FILE" 2>/dev/null || stat -f "%p" "$SECRET_FILE" 2>/dev/null | tail -c 4)
    # Check it's 0600 (or 100600 on some systems)
    echo "$PERMS" | grep -qE "600$" \
        || { echo "WARN: file perms are $PERMS (expected 600)"; }
else
    echo "FAIL: secret file not created at $SECRET_FILE"; exit 1
fi

# ── get (redacted) ───────────────────────────────────────────────────────────
HOME="$SMOKE_HOME" wg secret get "$SECRET_NAME" --backend plaintext 2>&1 | grep -q "exists:" \
    || { echo "FAIL: wg secret get plaintext did not show key exists"; exit 1; }

# get --reveal
REVEALED=$(HOME="$SMOKE_HOME" wg secret get "$SECRET_NAME" --backend plaintext --reveal 2>/dev/null)
[ "$REVEALED" = "$SECRET_VALUE" ] \
    || { echo "FAIL: --reveal returned '$REVEALED', expected '$SECRET_VALUE'"; exit 1; }

# ── list ─────────────────────────────────────────────────────────────────────
HOME="$SMOKE_HOME" wg secret list 2>&1 | grep -q "plain:${SECRET_NAME}" \
    || { echo "FAIL: wg secret list did not include plain secret"; exit 1; }

# ── rm ───────────────────────────────────────────────────────────────────────
HOME="$SMOKE_HOME" wg secret rm "$SECRET_NAME" --backend plaintext 2>&1 \
    | grep -q "deleted from plaintext backend" \
    || { echo "FAIL: wg secret rm plaintext did not report success"; exit 1; }

# File should be gone
[ ! -f "$SECRET_FILE" ] \
    || { echo "FAIL: secret file still exists after rm"; exit 1; }

echo "PASS: plaintext backend round-trip OK"
exit 0
