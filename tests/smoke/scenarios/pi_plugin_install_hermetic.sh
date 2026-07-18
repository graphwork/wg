#!/usr/bin/env bash
# Scenario: pi_plugin_install_hermetic
#
# Pins implement-pi-plugin: the `ensure-pi-plugin` install primitive and the
# hermetic `pi -e <cache>/pi-worksgood/index.js -ne` load.
#
# Always-on (pure `wg` binary, no node/pi/creds):
#   * `wg pi-plugin compat-version` prints the embedded compat version.
#   * `wg pi-plugin install` (cache path) materializes the embedded bundle into
#     the versioned cache, writes the `~/.pi/agent/settings.json` extensions
#     entry, and is IDEMPOTENT (a second run is byte-identical).
#   * A corrupted cache is DETECTED and self-HEALED on the next install.
#
# Live (only when a `pi` binary is present — no credentials needed because the
# extension loads at pi startup before any model connection):
#   * `pi --mode rpc -e <cache dist> -ne` with a MISMATCHED
#     WG_PI_PLUGIN_COMPAT_VERSION fails LOUDLY with a compat-mismatch error
#     naming plugin-vs-wg versions (the loud-fail tripwire through REAL pi).
#   * the same with a MATCHING version loads cleanly (no load error).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
cache="$scratch/cache"
mkdir -p "$fake_home" "$cache"

# Isolated, idempotent invocation: fake HOME + XDG_CACHE_HOME, and
# WG_PI_PLUGIN_FORCE_CACHE so we exercise the embedded→cache (user) path even
# when the smoke gate runs from inside a checkout (where Dev source would win).
run_pi_plugin() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_PI_PLUGIN_DIR \
        HOME="$fake_home" XDG_CACHE_HOME="$cache" WG_PI_PLUGIN_FORCE_CACHE=1 \
        wg pi-plugin "$@"
}

# ── compat-version ──────────────────────────────────────────────────
compat=$(run_pi_plugin compat-version 2>/dev/null) \
    || loud_fail "wg pi-plugin compat-version failed"
[ -n "$compat" ] || loud_fail "wg pi-plugin compat-version printed nothing"

# ── install #1 (cache path) ─────────────────────────────────────────
out1=$(run_pi_plugin install 2>&1) || loud_fail "wg pi-plugin install failed: $out1"

settings="$fake_home/.pi/agent/settings.json"
[ -f "$settings" ] || loud_fail "install did not write $settings"
cache_dist="$cache/wg/worksgood-pi/$compat/pi-worksgood/index.js"
[ -f "$cache_dist" ] || loud_fail "install did not extract embedded bundle to $cache_dist"
[ -f "$cache/wg/worksgood-pi/$compat/.wg-ok" ] || loud_fail "install did not write the .wg-ok integrity stamp"
grep -qF "$cache_dist" "$settings" \
    || loud_fail "settings.json does not list the cache extension entry. Got: $(cat "$settings")"

# ── install #2 must be a verified no-op (idempotent) ────────────────
sum1=$(sha256sum "$settings" | cut -d' ' -f1)
run_pi_plugin install >/dev/null 2>&1 || loud_fail "second install failed"
sum2=$(sha256sum "$settings" | cut -d' ' -f1)
[ "$sum1" = "$sum2" ] || loud_fail "install is NOT idempotent — settings.json changed on the second run"

# ── corrupted cache is detected + repaired (self-heal) ──────────────
printf '// CORRUPTED\n' > "$cache_dist"
rm -f "$cache/wg/worksgood-pi/$compat/.wg-ok"
ready_bad=$(run_pi_plugin status 2>/dev/null | grep -i "build ready")
grep -qi "NO" <<<"$ready_bad" || loud_fail "corrupted cache was not detected as not-ready: $ready_bad"
run_pi_plugin install >/dev/null 2>&1 || loud_fail "repair install failed"
if head -c 32 "$cache_dist" | grep -q CORRUPTED; then
    loud_fail "corrupted cache was NOT repaired (index.js still corrupted)"
fi
[ -f "$cache/wg/worksgood-pi/$compat/.wg-ok" ] || loud_fail "repair did not restore the .wg-ok stamp"

# ── Live hermetic load through REAL pi (only if a pi binary exists) ──
if command -v pi >/dev/null 2>&1; then
    # Mismatch → loud compat error at extension load (no credentials needed).
    mism=$(printf '{"type":"prompt","message":"hi"}\n' \
        | timeout "${WG_SMOKE_TIMEOUT_SECS:-30}" \
            env -u OPENROUTER_API_KEY -u ANTHROPIC_API_KEY \
                WG_PI_PLUGIN_COMPAT_VERSION="9.9.9-smoke-mismatch" \
            pi --mode rpc -e "$cache_dist" -ne \
               --provider openrouter --model anthropic/claude-3.5-haiku --no-approve 2>&1)
    grep -qi "compat mismatch" <<<"$mism" \
        || loud_fail "real pi did not surface the loud compat-mismatch tripwire. Output: $mism"
    grep -qi "9.9.9-smoke-mismatch" <<<"$mism" \
        || loud_fail "compat-mismatch error did not name the found (wg) version. Output: $mism"

    # Matching version → loads cleanly (no extension load error).
    okrun=$(printf '{"type":"shutdown"}\n' \
        | timeout "${WG_SMOKE_TIMEOUT_SECS:-30}" \
            env -u OPENROUTER_API_KEY -u ANTHROPIC_API_KEY \
                WG_PI_PLUGIN_COMPAT_VERSION="$compat" \
            pi --mode rpc -e "$cache_dist" -ne \
               --provider openrouter --model anthropic/claude-3.5-haiku --no-approve 2>&1)
    if grep -qi "Failed to load extension" <<<"$okrun"; then
        loud_fail "matching-version plugin failed to load via pi -e. Output: $okrun"
    fi
    echo "pi_plugin_install_hermetic: real-pi hermetic load + loud-mismatch checks passed"
else
    echo "pi_plugin_install_hermetic: no \`pi\` binary — skipped the live -e/-ne load check (core install/idempotency/repair all passed)"
fi

echo "pi_plugin_install_hermetic: PASS (compat=$compat)"
exit 0
