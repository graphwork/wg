#!/usr/bin/env bash
# Scenario: profile_use_preserve_openrouter_endpoint
#
# Regression lock for bug-profile-use-preserve-openrouter-endpoint.
#
# Pre-fix, `wg profile use <name>` copied `~/.wg/profiles/<name>.toml` over
# `~/.wg/config.toml` byte-for-byte, which CLOBBERED a configured OpenRouter
# endpoint / credential (the `pi`, `claude`, and `codex` starters carry no
# `[llm_endpoints]`, so the endpoint set by `wg login openrouter --global`
# vanished). The pinned-model path's `Config::save_global` round-trip also
# REINTRODUCED deprecated `dispatcher.poll_interval` and removed compaction/
# verify keys with their serde defaults.
#
# Post-fix, profile activation is an OVERLAY: the profile's routing keys are
# merged onto the existing GLOBAL config (preserving `[[llm_endpoints]]`
# credentials/endpoints), and the merged file is canonicalized so it is
# lint-clean. (Project-LOCAL routing overrides are still cleared, by design —
# local must not shadow the profile's global routing — but the GLOBAL endpoint
# set by `wg login openrouter --global` survives.)
#
# This scenario is credential-free: it seeds the GLOBAL config with an
# OpenRouter endpoint (api_key_ref) + the deprecated keys `wg login openrouter
# --global`'s `save_global` round-trip writes, applies the `pi` profile from a
# separate worktree repo, and asserts the GLOBAL endpoint SURVIVES and the
# GLOBAL config is CANONICAL.
#
# Pure config-state correctness check; no LLM call, no network.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

# Isolate HOME so the GLOBAL config dir is $scratch/.wg (NOT the host's ~/.wg).
export HOME="$scratch"
GLOBAL_DIR="$scratch/.wg"
mkdir -p "$GLOBAL_DIR"

# A separate worktree repo so the project-LOCAL .wg/config.toml is distinct
# from the GLOBAL config (profile activation clears LOCAL routing overrides by
# design; the OpenRouter endpoint under test lives in the GLOBAL config).
repo="$scratch/repo"
mkdir -p "$repo"
cd "$repo"
if ! wg init -m claude:opus >init.log 2>&1; then
    loud_fail "wg init failed in repo dir: $(tail -5 init.log)"
fi

# ── 1. Seed the GLOBAL config with an OpenRouter endpoint + deprecated keys ──
# This mirrors what `wg login openrouter --global`'s `save_global` round-trip
# writes: the OpenRouter endpoint entry PLUS the deprecated `poll_interval` /
# removed compaction/verify keys the round-trip re-emits with serde defaults.
cat > "$GLOBAL_DIR/config.toml" <<'TOML'
[agent]
model = "claude:opus"

[dispatcher]
model = "claude:opus"
poll_interval = 5
compaction_token_threshold = 50000
verify_autospawn_enabled = true
verify_mode = "separate"

[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_ref = "keyring:openrouter"
is_default = true
TOML

# Sanity: the endpoint is configured in the GLOBAL config before the swap.
grep -q 'api_key_ref = "keyring:openrouter"' "$GLOBAL_DIR/config.toml" \
    || loud_fail "OpenRouter endpoint missing from GLOBAL config BEFORE profile swap: $(cat "$GLOBAL_DIR/config.toml")"

# ── 2. Apply the pi profile (the regression trigger) ─────────────────────────
# `wg profile use pi` must NOT clobber the GLOBAL OpenRouter endpoint and must
# NOT reintroduce deprecated/removed keys. --no-reload so it doesn't poke a
# daemon (none running in the smoke env).
profile_out=$(wg profile use pi --no-reload 2>&1) \
    || loud_fail "wg profile use pi failed: $profile_out"
echo "$profile_out"

# ── 3. The GLOBAL OpenRouter endpoint must survive the pi swap ───────────────
gcfg="$GLOBAL_DIR/config.toml"
grep -q 'api_key_ref = "keyring:openrouter"' "$gcfg" \
    || loud_fail "api_key_ref dropped from GLOBAL config.toml by profile swap: $(cat "$gcfg")"
grep -q 'https://openrouter.ai/api/v1' "$gcfg" \
    || loud_fail "OpenRouter URL dropped from GLOBAL config.toml by profile swap: $(cat "$gcfg")"
grep -q 'name = "openrouter"' "$gcfg" \
    || loud_fail "OpenRouter endpoint name dropped from GLOBAL config.toml by profile swap: $(cat "$gcfg")"

# The merged view (global + local) must still surface the OpenRouter endpoint.
# After the swap the active profile is `pi`, so local inherits global endpoints
# (local declares none); `wg endpoints list` must report openrouter.
post_endpoints=$(wg endpoints list --json 2>&1) \
    || loud_fail "wg endpoints list failed after profile swap: $post_endpoints"
echo "$post_endpoints" | grep -q '"name": "openrouter"' \
    || loud_fail "OpenRouter endpoint CLOBBERED from merged view by wg profile use pi: $post_endpoints"
echo "$post_endpoints" | grep -q '"provider": "openrouter"' \
    || loud_fail "OpenRouter provider lost from merged view after profile swap: $post_endpoints"

# The pi profile's routing must take effect in the GLOBAL config.
grep -q 'pi:openrouter/z-ai/glm-5.2' "$gcfg" \
    || loud_fail "pi strong-tier route missing after profile swap: $(cat "$gcfg")"

# ── 4. The GLOBAL config must be canonical (no deprecated/removed keys) ──────
grep -q 'poll_interval' "$gcfg" \
    && loud_fail "deprecated dispatcher.poll_interval REINTRODUCED by profile swap: $(cat "$gcfg")"
grep -Eq 'compaction_token_threshold|compactor_interval|verify_autospawn_enabled|verify_mode' "$gcfg" \
    && loud_fail "removed compaction/verify keys REINTRODUCED by profile swap: $(cat "$gcfg")"

# `wg config lint --global` migrate findings must be empty (canonical config).
lint_json=$(wg config lint --global --json 2>&1) \
    || loud_fail "wg config lint --global failed: $lint_json"
echo "$lint_json" | python3 -c '
import json, sys
data = json.load(sys.stdin)
findings = 0
for f in data.get("files", []):
    findings += len(f.get("removed_keys", [])) \
              + len(f.get("renamed_keys", [])) \
              + len(f.get("rewritten_values", []))
if findings != 0:
    print("NON-CANONICAL (global): migrate findings = %d" % findings, file=sys.stderr)
    print(json.dumps(data, indent=2), file=sys.stderr)
    sys.exit(1)
print("canonical (global): 0 migrate findings")
'

# `wg config lint --local` migrate findings must also be empty — the local
# config written by `wg init` carries deprecated `poll_interval`/compaction/
# verify keys (Config::save serializes the deprecated field names); profile
# activation must canonicalize the local too so the merged config deserializes
# without "duplicate field poll_interval" errors.
lint_local=$(wg config lint --local --json 2>&1) \
    || loud_fail "wg config lint --local failed: $lint_local"
echo "$lint_local" | python3 -c '
import json, sys
data = json.load(sys.stdin)
findings = 0
for f in data.get("files", []):
    findings += len(f.get("removed_keys", [])) \
              + len(f.get("renamed_keys", [])) \
              + len(f.get("rewritten_values", []))
if findings != 0:
    print("NON-CANONICAL (local): migrate findings = %d" % findings, file=sys.stderr)
    print(json.dumps(data, indent=2), file=sys.stderr)
    sys.exit(1)
print("canonical (local): 0 migrate findings")
'

echo ""
echo "PASS: wg profile use pi preserved the GLOBAL OpenRouter endpoint and wrote a canonical config."
