#!/usr/bin/env bash
# Scenario: pi_install_e2e
#
# End-to-end guard for the pi<->wg install technique (test-end-to), covering the
# declarative/lifecycle wiring NOT already pinned by pi_plugin_install_hermetic
# (which exercises `wg pi-plugin install` + raw `pi -e`). This pins the three
# real lifecycle entry points and the node-free promise:
#
# Always-on (pure `wg` binary, no node/npm/pi, no credentials):
#   1. PROFILE-DRIVEN install (scenario 2): from a CLEAN ~/.pi + cache,
#      `wg profile use pi` ensures the plugin as an idempotent side effect —
#      settings.json is wired to the versioned cache dist, version-matched, and
#      `wg pi-plugin status` reports console-wired. A second `profile use pi` is
#      byte-identical (idempotent / scenario 4).
#   2. NODE-LESS install (scenario 6): with NO node/npm/pi anywhere on PATH,
#      `wg pi-plugin install` still materializes the EMBEDDED bytes into the
#      cache and wires settings (proves `cargo install` users need no toolchain).
#
# Live, credential-free (only when a `pi` binary is present — the plugin loads
# at pi startup before any model connection):
#   3. HERMETIC HANDLER SPAWN (scenario 1, the primary reliability claim): from a
#      CLEAN ~/.pi, `wg pi-handler` ensures the plugin from the cache (source=Cache)
#      and spawns `pi --mode rpc … -e <cache>/<compat>/pi-worksgood/index.js -ne` — and
#      NEVER writes a global `~/.pi/agent/settings.json` plugin entry (hermetic).
#   4. LOUD COMPAT MISMATCH (scenario 5): a version-skewed plugin makes real
#      `pi --mode rpc -e <cache dist> -ne` fail LOUDLY and early with a
#      compat-mismatch error naming the skewed wg version — proving no silent
#      drift of the class that caused the glm-5.2 401.
#
# The live wg-verb round-trips (a model calling wg_add/wg_done back into the
# graph) are credentialed + model-nondeterministic and live in the e2e REPORT
# (docs/pi-install-e2e-report.md) / pi_plugin_live_validation, not this gate.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

compat=$(wg pi-plugin compat-version 2>/dev/null) || loud_fail "wg pi-plugin compat-version failed"
[ -n "$compat" ] || loud_fail "wg pi-plugin compat-version printed nothing"

# Strip inherited worker env so we never touch the global daemon / dev source.
clean_env() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        -u WG_DIR -u WG_PROJECT_DIR -u WG_PI_PLUGIN_DIR "$@"
}

# ── 1. PROFILE-DRIVEN install (scenario 2) ──────────────────────────
s2=$(make_scratch)
mkdir -p "$s2/home" "$s2/cache" "$s2/proj"
[ -e "$s2/home/.pi" ] && loud_fail "scratch HOME not clean (~/.pi already present)"

( cd "$s2/proj" && clean_env HOME="$s2/home" XDG_CACHE_HOME="$s2/cache" \
    XDG_CONFIG_HOME="$s2/home/.config" \
    wg init -m openrouter:anthropic/claude-3.5-haiku --no-agency >/dev/null 2>&1 ) \
    || loud_fail "wg init (scenario 2 project) failed"

prof_out=$( cd "$s2/proj" && clean_env HOME="$s2/home" XDG_CACHE_HOME="$s2/cache" \
    XDG_CONFIG_HOME="$s2/home/.config" WG_PI_PLUGIN_FORCE_CACHE=1 \
    wg profile use pi 2>&1 ) || loud_fail "wg profile use pi failed: $prof_out"

grep -qi "Ensured pi-worksgood" <<<"$prof_out" \
    || loud_fail "'wg profile use pi' did not declaratively ensure the plugin. Got: $prof_out"

settings="$s2/home/.pi/agent/settings.json"
cache_dist="$s2/cache/wg/worksgood-pi/$compat/pi-worksgood/index.js"
[ -f "$settings" ]   || loud_fail "profile use pi did not write $settings"
[ -f "$cache_dist" ] || loud_fail "profile use pi did not materialize $cache_dist"
grep -qF "$cache_dist" "$settings" \
    || loud_fail "settings.json is not wired to the cache dist. Got: $(cat "$settings")"

# version-matched
got_compat=$(grep -o '"compat"[^,}]*' "$s2/cache/wg/worksgood-pi/$compat/version.json" 2>/dev/null)
grep -qF "$compat" <<<"$got_compat" \
    || loud_fail "cache version.json not matched to compat $compat: $got_compat"

# status reports console-wired
status_out=$( cd "$s2/proj" && clean_env HOME="$s2/home" XDG_CACHE_HOME="$s2/cache" \
    XDG_CONFIG_HOME="$s2/home/.config" WG_PI_PLUGIN_FORCE_CACHE=1 wg pi-plugin status 2>&1 )
grep -qiE "console wired: *yes" <<<"$status_out" \
    || loud_fail "wg pi-plugin status does not report console-wired after profile use. Got: $status_out"

# idempotent re-run (scenario 4)
sum1=$(sha256sum "$settings" | cut -d' ' -f1)
( cd "$s2/proj" && clean_env HOME="$s2/home" XDG_CACHE_HOME="$s2/cache" \
    XDG_CONFIG_HOME="$s2/home/.config" WG_PI_PLUGIN_FORCE_CACHE=1 \
    wg profile use pi >/dev/null 2>&1 ) || loud_fail "second profile use pi failed"
sum2=$(sha256sum "$settings" | cut -d' ' -f1)
[ "$sum1" = "$sum2" ] || loud_fail "profile use pi is NOT idempotent — settings.json changed on the second run"

echo "pi_install_e2e: [2] profile-driven Console install + version-match + idempotency PASS"

# ── 2. NODE-LESS install (scenario 6) ───────────────────────────────
s6=$(make_scratch)
mkdir -p "$s6/home" "$s6/cache" "$s6/bin"
# A minimal PATH with coreutils + wg but explicitly NO node/npm/pi.
for t in env bash sh cat ls grep sed mkdir rm sha256sum dirname basename head cut; do
    src=$(command -v "$t" 2>/dev/null) && ln -sf "$src" "$s6/bin/$t"
done
wgbin=$(command -v wg); ln -sf "$wgbin" "$s6/bin/wg"
if PATH="$s6/bin" command -v node >/dev/null 2>&1 \
   || PATH="$s6/bin" command -v npm >/dev/null 2>&1 \
   || PATH="$s6/bin" command -v pi >/dev/null 2>&1; then
    loud_fail "node-less PATH is not actually node-less (node/npm/pi leaked into $s6/bin)"
fi
nl_out=$(PATH="$s6/bin" clean_env HOME="$s6/home" XDG_CACHE_HOME="$s6/cache" \
    WG_PI_PLUGIN_FORCE_CACHE=1 wg pi-plugin install 2>&1) \
    || loud_fail "node-less wg pi-plugin install failed (should be node-free): $nl_out"
[ -f "$s6/cache/wg/worksgood-pi/$compat/pi-worksgood/index.js" ] \
    || loud_fail "node-less install did not extract the embedded bundle"
[ -f "$s6/cache/wg/worksgood-pi/$compat/.wg-ok" ] \
    || loud_fail "node-less install did not write the .wg-ok stamp"
[ -f "$s6/home/.pi/agent/settings.json" ] \
    || loud_fail "node-less install did not wire settings.json"
echo "pi_install_e2e: [6] node-less embedded install PASS"

# ── 3. + 4. LIVE pi sub-checks (only with a pi binary; credential-free) ──
if command -v pi >/dev/null 2>&1; then
    # 3. Hermetic `wg pi-handler` spawn from a CLEAN ~/.pi.
    sh=$(make_scratch)
    mkdir -p "$sh/home" "$sh/cache" "$sh/proj"
    ( cd "$sh/proj" && clean_env HOME="$sh/home" XDG_CACHE_HOME="$sh/cache" \
        XDG_CONFIG_HOME="$sh/home/.config" \
        wg init -m openrouter:anthropic/claude-3.5-haiku --no-agency >/dev/null 2>&1 ) \
        || loud_fail "wg init (handler project) failed"
    wgdir="$sh/proj/.wg"
    hlog="$wgdir/chat/chat-1/handler.log"

    # WG_PI_PLUGIN_FORCE_CACHE=1 forces the embedded→cache (user) source even when
    # the smoke runs from inside the wg checkout (where the Dev source would win).
    ( cd "$sh/proj" && clean_env HOME="$sh/home" XDG_CACHE_HOME="$sh/cache" \
        XDG_CONFIG_HOME="$sh/home/.config" WG_PI_PLUGIN_FORCE_CACHE=1 \
        timeout 25 wg pi-handler --chat chat-1 -m pi:openrouter/openai/gpt-4o-mini \
        >"$sh/handler.out" 2>&1 ) &
    hpid=$!
    for _ in $(seq 1 20); do
        [ -f "$hlog" ] && grep -q "spawning" "$hlog" 2>/dev/null && break
        sleep 1
    done
    kill "$hpid" 2>/dev/null
    # Reap the timeout + its child pi (best-effort: kill the process group).
    pkill -P "$hpid" 2>/dev/null
    wait "$hpid" 2>/dev/null

    [ -f "$hlog" ] || loud_fail "pi-handler wrote no handler.log"
    grep -q "ensured plugin source=Cache" "$hlog" \
        || loud_fail "pi-handler did not ensure the plugin from the CACHE. Log: $(cat "$hlog")"
    sh_cache_dist="$sh/cache/wg/worksgood-pi/$compat/pi-worksgood/index.js"
    grep -qF -- "-e $sh_cache_dist -ne" "$hlog" \
        || loud_fail "pi-handler spawn argv missing the hermetic '-e <cache dist> -ne'. Log: $(cat "$hlog")"
    # Hermetic: the wg→pi direction must NEVER write a global pi plugin entry.
    [ -f "$sh/home/.pi/agent/settings.json" ] \
        && loud_fail "wg pi-handler wrote a global ~/.pi/agent/settings.json — NOT hermetic"
    echo "pi_install_e2e: [1] hermetic wg pi-handler spawn (source=Cache, -e<cache>-ne, no ~/.pi wiring) PASS"

    # 4. Loud compat mismatch through real pi (no credentials needed).
    sm=$(make_scratch)
    mkdir -p "$sm/home" "$sm/cache"
    clean_env HOME="$sm/home" XDG_CACHE_HOME="$sm/cache" WG_PI_PLUGIN_FORCE_CACHE=1 \
        wg pi-plugin install >/dev/null 2>&1 || loud_fail "install for mismatch check failed"
    sm_dist="$sm/cache/wg/worksgood-pi/$compat/pi-worksgood/index.js"
    mism=$(printf '{"type":"prompt","message":"hi"}\n' | timeout "${WG_SMOKE_TIMEOUT_SECS:-30}" \
        env -u OPENROUTER_API_KEY -u ANTHROPIC_API_KEY \
            WG_PI_PLUGIN_COMPAT_VERSION="9.9.9-e2e-skew" \
        pi --mode rpc -e "$sm_dist" -ne \
           --provider openrouter --model openai/gpt-4o-mini --no-approve 2>&1)
    grep -qi "compat mismatch" <<<"$mism" \
        || loud_fail "real pi did not surface the loud compat-mismatch tripwire. Output: $mism"
    grep -qi "9.9.9-e2e-skew" <<<"$mism" \
        || loud_fail "compat-mismatch error did not name the skewed wg version. Output: $mism"
    echo "pi_install_e2e: [5] loud compat-mismatch through real pi PASS"
else
    echo "pi_install_e2e: no \`pi\` binary — SKIPPED the live hermetic-spawn + compat-mismatch sub-checks (install/profile/node-less core all passed)"
fi

echo "pi_install_e2e: PASS (compat=$compat)"
exit 0
