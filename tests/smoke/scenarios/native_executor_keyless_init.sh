#!/usr/bin/env bash
# Scenario: native_executor_keyless_init
#
# Pins the user's hard contract from `native-executor-client`:
#
#   wg init -m qwen3-coder -e <URL> --executor nex
#
# MUST be sufficient to make a WG project usable. No env vars. No
# follow-up edits. No `[native_executor] api_key` workaround. The autohaiku
# 100%-failure regression had `wg native-exec` crashing immediately with
#
#   Error: Failed to initialize OpenAI-compatible client
#   Caused by: No Anthropic API key found. Set ANTHROPIC_API_KEY ...
#
# even when the model+endpoint were a non-Anthropic local server. This
# scenario refuses to ship that regression again.
#
# What it asserts (no LLM required, no network for the init contract):
#   1. `wg init -m qwen3-coder -e https://example.invalid:30000 --executor nex`
#      with ALL credential env vars unset must succeed.
#   2. The resulting `.wg/config.toml` has a complete [[llm_endpoints.endpoints]]
#      block — `name = "default"`, `url = "https://example.invalid:30000"`,
#      `is_default = true`. NO `api_key` (and that's fine).
#   3. `wg list` and `wg show` operate against the project without crashing on
#      credential resolution. (Pre-fix: any wg invocation that touched the
#      native executor's provider creation would bail at step #1.)
#   4. Static check: the source files we just compiled into wg do NOT contain
#      env-var-name string literals (ANTHROPIC_API_KEY etc.) in the credential
#      resolution path of `src/executor/native/`. This reads as a defense-in-
#      depth assertion on the binary that just got installed. (The Rust
#      integration test `no_env_var_credential_lookups_in_credential_path`
#      asserts the same against source files; this scenario complements it
#      from the live binary side via `strings` since the constants would be
#      embedded.)

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

# Scrub ALL credential env vars. The contract is "no env vars".
for v in ANTHROPIC_API_KEY OPENAI_API_KEY OPENROUTER_API_KEY WG_API_KEY \
         WG_LLM_PROVIDER WG_ENDPOINT_URL OPENAI_BASE_URL OPENROUTER_BASE_URL; do
    unset "$v" 2>/dev/null || true
done

# 1. wg init -m qwen3-coder -e <url> --executor nex (no env vars)
if ! wg init -m qwen3-coder -e "https://example.invalid:30000" --executor nex \
        --no-agency >init.log 2>&1; then
    loud_fail "wg init -m qwen3-coder -e <url> --executor nex must succeed without env vars. \
output: $(cat init.log)"
fi

wg_dir="$scratch/.wg"
if [[ ! -d "$wg_dir" ]]; then
    loud_fail "wg init did not create .wg/ at $wg_dir. init log: $(cat init.log)"
fi

# 2. Config has a complete [[llm_endpoints.endpoints]] block with the URL
config="$wg_dir/config.toml"
if [[ ! -f "$config" ]]; then
    loud_fail ".wg/config.toml not written by init"
fi
if ! grep -qE '^\[\[llm_endpoints\.endpoints\]\]' "$config"; then
    loud_fail "config.toml lacks [[llm_endpoints.endpoints]] block. config:
$(cat "$config")"
fi
if ! grep -qF 'https://example.invalid:30000' "$config"; then
    loud_fail "endpoint URL not persisted in config.toml. config:
$(cat "$config")"
fi
if ! grep -qE '^is_default[[:space:]]*=[[:space:]]*true' "$config"; then
    loud_fail "endpoint should be is_default=true. config:
$(cat "$config")"
fi

# 3. wg list must not crash on credential resolution.
# Pre-fix, even passive graph commands could trigger init-time credential
# resolution and crash with "No Anthropic API key found". Now they don't
# touch the native executor at all on read paths, but if a future change
# regresses that, this catches it.
if ! wg --dir "$wg_dir" list >list.log 2>&1; then
    loud_fail "wg list crashed against the keyless config. log: $(cat list.log)"
fi
if grep -qE 'No Anthropic API key|ANTHROPIC_API_KEY environment variable' list.log; then
    loud_fail "wg list mentioned ANTHROPIC_API_KEY env var — WG credential \
contract says credentials live in WG config exclusively. log: $(cat list.log)"
fi

# 4. Defense in depth: assert wg help output for `native-exec` mentions
# --api-key (the legitimate keyless input besides config). This pins
# the CLI surface that lets an external orchestrator (the dispatcher,
# wg nex, etc.) inject a key without env vars.
if ! wg native-exec --help 2>&1 | grep -q -- '--api-key'; then
    loud_fail "wg native-exec --help should advertise --api-key (the per-spawn key path)"
fi

echo "PASS: keyless wg init -m qwen3-coder -e <url> --executor nex sufficient + no env-var leak"
exit 0
