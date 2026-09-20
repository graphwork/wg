#!/usr/bin/env bash
# Scenario: pi_plugin_cache_embed_digest
#
# Pins fix-plugin-cache: the wg-pi plugin cache had silently served a
# six-week-stale build, so shipped features (completion wakeups + the VizView
# panel) never reached any live pi session. Three defects, one failure mode:
#
#   1. cache_is_correct() validated only file EXISTENCE + the compat string, so
#      an embed change that did not bump WG_PI_PLUGIN_COMPAT_VERSION left the
#      stale cache in place forever. The cache now carries a `.wg-embed-digest`
#      content stamp and cache_is_correct recomputes + compares content identity.
#   2. pick_source() preferred a compile-time Dev tree even when it lived under
#      a prunable `.wg-worktrees/` path (a `cargo install --path .` from a
#      worktree baked that path into the global binary).
#   3. `wg pi-plugin status` reported "build ready: yes" while the cache was
#      stale; it must now report cache-vs-embed drift.
#
# Credential-free, pure `wg` binary, isolated HOME/XDG_CACHE_HOME.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
cache="$scratch/cache"
mkdir -p "$fake_home" "$cache"

run_pi_plugin() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_PI_PLUGIN_DIR \
        HOME="$fake_home" XDG_CACHE_HOME="$cache" WG_PI_PLUGIN_FORCE_CACHE=1 \
        wg pi-plugin "$@"
}

compat=$(run_pi_plugin compat-version 2>/dev/null) \
    || loud_fail "wg pi-plugin compat-version failed"
[ -n "$compat" ] || loud_fail "wg pi-plugin compat-version printed nothing"

cache_dir="$cache/wg/worksgood-pi/$compat"
cache_dist="$cache_dir/pi-worksgood/index.js"

# ── install materializes the shipped features + content stamp ───────
out1=$(run_pi_plugin install 2>&1) || loud_fail "wg pi-plugin install failed: $out1"
[ -f "$cache_dist" ] || loud_fail "install did not extract embedded bundle to $cache_dist"

for shipped in pi-worksgood/completion-watcher.js pi-worksgood/viz-panel.js; do
    [ -f "$cache_dir/$shipped" ] \
        || loud_fail "re-extracted cache missing shipped feature file $shipped"
done
[ -f "$cache_dir/.wg-embed-digest" ] \
    || loud_fail "install did not write the .wg-embed-digest content stamp"
[ -s "$cache_dir/.wg-embed-digest" ] \
    || loud_fail ".wg-embed-digest content stamp is empty"

# ── status reports a current cache ──────────────────────────────────
status_ok=$(run_pi_plugin status 2>&1) || loud_fail "wg pi-plugin status failed"
grep -qi "cache state:.*current" <<<"$status_ok" \
    || loud_fail "status did not report a current cache: $status_ok"
grep -qi "embed digest:" <<<"$status_ok" \
    || loud_fail "status did not report the embed digest: $status_ok"

# ── node --check on the entry (only when node is available) ─────────
if command -v node >/dev/null 2>&1; then
    node --check "$cache_dist" \
        || loud_fail "node --check failed on the materialized entry $cache_dist"
    echo "pi_plugin_cache_embed_digest: node --check passed on $cache_dist"
else
    echo "pi_plugin_cache_embed_digest: no node binary — skipped node --check (cache content asserted instead)"
fi

# ── simulate the six-week-stale cache: content no longer matches the embed ──
# Same compat version, but the stamped digest is bogus AND a shipped file is
# missing. An existence-only predicate would call this "correct"; the content
# digest must call it DRIFT.
printf 'b3:000000000000000000000000000000stale\n' > "$cache_dir/.wg-embed-digest"
rm -f "$cache_dir/pi-worksgood/completion-watcher.js"

drift_status=$(run_pi_plugin status 2>&1) || loud_fail "wg pi-plugin status failed on drift"
grep -qi "cache state:.*DRIFTED" <<<"$drift_status" \
    || loud_fail "status did not report cache DRIFT: $drift_status"
grep -qi "build ready:.*NO" <<<"$drift_status" \
    || loud_fail "drifted cache still reported build ready under Cache source: $drift_status"
grep -qi "WARNING" <<<"$drift_status" \
    || loud_fail "status did not warn about the drifted cache: $drift_status"

# ── install re-materializes the embedded truth (self-heal) ──────────
run_pi_plugin install >/dev/null 2>&1 || loud_fail "repair install failed"
[ -f "$cache_dir/pi-worksgood/completion-watcher.js" ] \
    || loud_fail "repair did not restore the missing shipped feature file"
grep -q "b3:0000" "$cache_dir/.wg-embed-digest" \
    && loud_fail "repair did not rewrite the stale digest stamp"

status_fixed=$(run_pi_plugin status 2>&1) || loud_fail "status failed after repair"
grep -qi "cache state:.*current" <<<"$status_fixed" \
    || loud_fail "cache not reported current after repair: $status_fixed"

# ── idempotent: a second install does not change the digest stamp ───
stamp1=$(sha256sum "$cache_dir/.wg-embed-digest" | cut -d' ' -f1)
run_pi_plugin install >/dev/null 2>&1 || loud_fail "second (no-op) install failed"
stamp2=$(sha256sum "$cache_dir/.wg-embed-digest" | cut -d' ' -f1)
[ "$stamp1" = "$stamp2" ] || loud_fail "no-op install rewrote the digest stamp"

echo "pi_plugin_cache_embed_digest: PASS (compat=$compat)"
exit 0
