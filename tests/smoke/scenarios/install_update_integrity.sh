#!/usr/bin/env bash
# Scenario: install_update_integrity
#
# Pins install-update-integrity: `wg pi-plugin install` must never be a SILENT
# no-op when `WG_PI_PLUGIN_DIR` points at the canonical cache dir — which is
# exactly how WG launches every spawned pi session. Before the fix, pick_source()
# took the EnvOverride branch and skipped materialize_cache()/content-digest
# validation entirely, so a stale cache was served forever and install reported
# success while changing nothing. This scenario pins:
#
#   1. install with WG_PI_PLUGIN_DIR=<cache>/worksgood-pi/<compat>, on a stale
#      cache, content-validates + re-materializes from the running binary's
#      embed and restores the shipped files + digest stamp;
#   2. on a matching cache the same invocation says EXPLICITLY that nothing
#      changed (never a bare success);
#   3. status with the same override reports the cache by health (current), not
#      by bare file existence;
#   4. `wg dev-sync --dry-run --no-install --no-restart` prints the verification
#      contract: binary path (+hash), embed digest, cache digest, daemon identity.
#
# Credential-free, pure `wg` binary, isolated HOME/XDG_CACHE_HOME/CARGO_HOME.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
cache="$scratch/cache"
cargo_home="$scratch/cargo-home"
mkdir -p "$fake_home" "$cache" "$cargo_home"

strip_env=(env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID
    -u WG_DIR -u WG_PROJECT_DIR -u WG_WORKTREE_PATH -u WG_PI_PLUGIN_DIR
    HOME="$fake_home" XDG_CACHE_HOME="$cache" CARGO_HOME="$cargo_home")

compat=$("${strip_env[@]}" wg pi-plugin compat-version 2>/dev/null) \
    || loud_fail "wg pi-plugin compat-version failed"
[ -n "$compat" ] || loud_fail "compat-version printed nothing"

cache_dir="$cache/wg/worksgood-pi/$compat"
cache_dist="$cache_dir/pi-worksgood/index.js"

# ── 1. materialize the embedded cache ───────────────────────────────
out0=$("${strip_env[@]}" WG_PI_PLUGIN_FORCE_CACHE=1 wg pi-plugin install 2>&1) \
    || loud_fail "initial install failed: $out0"
[ -f "$cache_dist" ] || loud_fail "install did not extract the embedded bundle"
[ -f "$cache_dir/.wg-embed-digest" ] || loud_fail "install did not write the digest stamp"
embed_stamp=$(tr -d '[:space:]' < "$cache_dir/.wg-embed-digest")

# ── 2. EnvOverride naming the cache dir, stale cache → refresh ──────
printf 'b3:000000000000000000000000000000stale\n' > "$cache_dir/.wg-embed-digest"
rm -f "$cache_dir/pi-worksgood/completion-watcher.js"

out1=$("${strip_env[@]}" WG_PI_PLUGIN_DIR="$cache_dir" wg pi-plugin install 2>&1) \
    || loud_fail "override install failed: $out1"
grep -qi "refreshed" <<<"$out1" \
    || loud_fail "override install did not report a cache refresh: $out1"
[ -f "$cache_dir/pi-worksgood/completion-watcher.js" ] \
    || loud_fail "override install did not restore the missing shipped file"
[ "$(tr -d '[:space:]' < "$cache_dir/.wg-embed-digest")" = "$embed_stamp" ] \
    || loud_fail "override install did not restore the matching digest stamp"

# ── 3. EnvOverride naming the cache dir, matching cache → explicit no-op ──
out2=$("${strip_env[@]}" WG_PI_PLUGIN_DIR="$cache_dir" wg pi-plugin install 2>&1) \
    || loud_fail "override no-op install failed: $out2"
grep -qi "already up to date" <<<"$out2" \
    || loud_fail "a matching cache must say 'already up to date', not report a bare success: $out2"
grep -qi "^Installed pi-worksgood" <<<"$out2" \
    && loud_fail "a matching cache must NOT claim it installed anything: $out2"

# ── 4. status with the same override reports cache health ───────────
status=$("${strip_env[@]}" WG_PI_PLUGIN_DIR="$cache_dir" wg pi-plugin status 2>&1) \
    || loud_fail "status with override failed"
grep -qi "cache state:.*current" <<<"$status" \
    || loud_fail "status did not report the cache current: $status"
grep -qi "build ready:.*yes" <<<"$status" \
    || loud_fail "status did not report build ready: yes: $status"

# ── 5. dev-sync verification block ──────────────────────────────────
# Run from a throwaway non-worktree git repo so dev-sync does not refuse.
devdir="$scratch/dev"
mkdir -p "$devdir"
git -C "$devdir" init -q 2>/dev/null || loud_fail "git init failed in $devdir"

devout=$(cd "$devdir" && "${strip_env[@]}" wg dev-sync --dry-run --no-install --no-restart 2>&1) \
    || loud_fail "wg dev-sync --dry-run failed: $devout"
for needle in "embed digest:" "cache digest:" "daemon:" "wg binary:"; do
    grep -q "$needle" <<<"$devout" \
        || loud_fail "dev-sync verification missing '$needle': $devout"
done
grep -qi "cache state:.*current" <<<"$devout" \
    || loud_fail "dev-sync did not report a current plugin cache: $devout"

echo "install_update_integrity: PASS (compat=$compat, embed=$embed_stamp)"
exit 0
