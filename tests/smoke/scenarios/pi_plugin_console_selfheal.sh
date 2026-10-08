#!/usr/bin/env bash
# Scenario: pi_plugin_console_selfheal
#
# Pins fix-console-plugin: a plain human `pi` console session must never
# silently run a plugin cache materialized by an OLDER `wg` binary. The console
# touchpoint the shipped plugin already shells at load (`wg pi-plugin
# compat-version`) now self-heals the binary-owned cache and rewires
# ~/.pi/agent/settings.json, and a new `wg pi-plugin digest` verb lets the
# plugin detect an embed change under an UNCHANGED compat version.
#
# Credential-free, pure `wg` binary, isolated HOME/XDG_CACHE_HOME, candidate
# binary preferred (falls back to target/debug/wg, then PATH `wg`).
#
# Checks:
#   1. an UNWIRED console is not bootstrapped by the query verb (no ~/.pi, no
#      cache extraction);
#   2. `wg pi-plugin digest` equals the materialized `.wg-embed-digest` stamp;
#   3. a CURRENT console is a silent, write-free no-op;
#   4. a STALE cache (same compat, changed content) self-heals from the running
#      binary's embed with a loud, actionable warning;
#   5. a stale wired version-dir path is rewired, unrelated entries preserved.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

REPO_ROOT="$(git -C "$HERE" rev-parse --show-toplevel 2>/dev/null || echo "$HERE/../..")"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" && -x "$WG_SMOKE_CANDIDATE_BIN" ]]; then
    WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
elif [[ -x "${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg" ]]; then
    WG_BIN="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/debug/wg"
else
    WG_BIN="$(command -v wg)"
fi

# The candidate must carry the console self-heal + the `digest` verb. An older
# installed binary on PATH cannot exercise this regression: skip LOUDLY rather
# than assert against the wrong binary.
if ! "$WG_BIN" pi-plugin digest >/dev/null 2>&1; then
    loud_skip "OLD WG BINARY" \
        "$WG_BIN lacks 'pi-plugin digest' (console self-heal); run 'wg dev-sync' or pass WG_SMOKE_CANDIDATE_BIN"
fi

scratch="$(make_scratch)"
fake_home="$scratch/home"
cache="$scratch/cache"
mkdir -p "$fake_home" "$cache"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_DIR -u WG_PROJECT_DIR -u WG_PI_PLUGIN_DIR \
        HOME="$fake_home" XDG_CACHE_HOME="$cache" WG_PI_PLUGIN_FORCE_CACHE=1 \
        "$WG_BIN" "$@"
}

settings="$fake_home/.pi/agent/settings.json"

# ── 1. unwired console is NOT bootstrapped by the query verb ────────
out=$(run_wg pi-plugin compat-version 2>/dev/null) || loud_fail "compat-version failed"
[ -n "$out" ] || loud_fail "compat-version printed nothing"
[ ! -e "$fake_home/.pi" ] \
    || loud_fail "compat-version bootstrapped a global ~/.pi on an unwired console"
[ ! -e "$cache/wg/worksgood-pi" ] \
    || loud_fail "compat-version materialized the cache on an unwired console"

# ── wire the console explicitly (install), then check the digest verb ─
run_wg pi-plugin install >/dev/null 2>&1 || loud_fail "initial install failed"
[ -f "$settings" ] || loud_fail "install did not wire $settings"
compat="$out"
cache_dir="$cache/wg/worksgood-pi/$compat"
dist="$cache_dir/pi-worksgood/index.js"
[ -f "$dist" ] || loud_fail "install did not extract the embedded bundle"

wg_digest=$(run_wg pi-plugin digest 2>/dev/null) || loud_fail "digest failed"
[ -n "$wg_digest" ] || loud_fail "digest printed nothing"
stamp=$(tr -d '[:space:]' < "$cache_dir/.wg-embed-digest")
[ "$wg_digest" = "$stamp" ] \
    || loud_fail "digest ($wg_digest) != materialized stamp ($stamp)"

# ── 2. current console: silent no-op, no cache/settings rewrite ─────
stamp_before=$(sha256sum "$cache_dir/.wg-embed-digest" | cut -d' ' -f1)
settings_before=$(sha256sum "$settings" | cut -d' ' -f1)
warn=$(run_wg pi-plugin compat-version 2>&1 >/dev/null)
[ -z "$warn" ] || loud_fail "a current console produced a warning: $warn"
[ "$stamp_before" = "$(sha256sum "$cache_dir/.wg-embed-digest" | cut -d' ' -f1)" ] \
    || loud_fail "a current console rewrote the cache"
[ "$settings_before" = "$(sha256sum "$settings" | cut -d' ' -f1)" ] \
    || loud_fail "a current console rewrote settings.json"

# ── 3. stale cache + fresh binary (same compat): heal + loud warning ─
printf 'b3:000000000000000000000000000000stale\n' > "$cache_dir/.wg-embed-digest"
rm -f "$cache_dir/pi-worksgood/completion-watcher.js"
warn=$(run_wg pi-plugin compat-version 2>&1 >/dev/null)
grep -qi "stale" <<<"$warn" \
    || loud_fail "a stale console did not warn: $warn"
grep -qi "restart pi" <<<"$warn" \
    || loud_fail "the stale warning is not actionable: $warn"
[ -f "$cache_dir/pi-worksgood/completion-watcher.js" ] \
    || loud_fail "self-heal did not restore the missing shipped file"
[ "$(tr -d '[:space:]' < "$cache_dir/.wg-embed-digest")" = "$wg_digest" ] \
    || loud_fail "self-heal did not restore the embed digest stamp"
stdout=$(run_wg pi-plugin compat-version 2>/dev/null)
[ "$stdout" = "$compat" ] \
    || loud_fail "compat-version stdout was polluted by the heal: $stdout"

# ── 4. stale wired version-dir path is rewired, unrelated kept ──────
cat > "$settings" <<JSON
{
  "extensions": [
    "/home/u/my-ext/index.ts",
    "/home/u/.cache/wg/worksgood-pi/0.0.1/pi-worksgood/index.js"
  ]
}
JSON
run_wg pi-plugin compat-version >/dev/null 2>&1 || loud_fail "rewire self-heal failed"
grep -qF "$dist" "$settings" \
    || loud_fail "stale version dir was not rewired to the current entry. Got: $(cat "$settings")"
grep -qF "/home/u/my-ext/index.ts" "$settings" \
    || loud_fail "unrelated extension entry was lost"
grep -qF "0.0.1" "$settings" \
    && loud_fail "stale version-dir entry survived the rewire"

echo "pi_plugin_console_selfheal: PASS (compat=$compat, digest=$wg_digest)"
exit 0
