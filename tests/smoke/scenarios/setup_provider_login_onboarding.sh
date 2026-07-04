#!/usr/bin/env bash
# Smoke: `wg setup` provider-login onboarding.
#
# Covers two high-risk paths:
#  1. `wg setup --route openrouter --scope local --from-stdin --backend keystore --yes`
#     must store/reference the credential via `api_key_ref`, never inline `api_key`.
#  2. `wg setup --route pi --scope local --yes` should reuse an existing GLOBAL
#     WG-managed OpenRouter login by setting `inherit_global = true` instead of
#     copying any secret into the repo-local config.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME"
unset WG_DIR ANTHROPIC_API_KEY OPENAI_API_KEY 2>/dev/null || true

# ── 1. Secret-backed OpenRouter onboarding from stdin ────────────────────────
proj_or="$scratch/openrouter-proj"
mkdir -p "$proj_or"
(
    cd "$proj_or"
    if ! printf '%s' 'sk-or-smoke-setup-test' | \
        wg setup --route openrouter --scope local --from-stdin --backend keystore --yes \
            >"$scratch/openrouter.log" 2>&1; then
        cat "$scratch/openrouter.log" 1>&2
        exit 1
    fi
) || loud_fail "openrouter setup onboarding failed: $(tail -20 "$scratch/openrouter.log")"

or_cfg="$proj_or/.wg/config.toml"
[[ -f "$or_cfg" ]] || loud_fail "expected repo-local config at $or_cfg"
grep -q 'api_key_ref = "keystore:openrouter"' "$or_cfg" \
    || loud_fail "openrouter setup did not write secret ref: $(cat "$or_cfg")"
if grep -q 'sk-or-smoke-setup-test' "$or_cfg"; then
    loud_fail "openrouter setup leaked the API key into config.toml"
fi
if grep -q 'api_key =' "$or_cfg"; then
    loud_fail "openrouter setup wrote inline api_key instead of api_key_ref"
fi

# ── 2. Pi repo-local onboarding reuses global WG OpenRouter login ────────────
mkdir -p "$HOME/.wg"
cat >"$HOME/.wg/config.toml" <<'EOF'
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_ref = "env:OPENROUTER_API_KEY"
is_default = true
EOF
export OPENROUTER_API_KEY="sk-or-smoke-global-reuse"

proj_pi="$scratch/pi-proj"
mkdir -p "$proj_pi"
(
    cd "$proj_pi"
    if ! wg setup --route pi --scope local --yes >"$scratch/pi.log" 2>&1; then
        cat "$scratch/pi.log" 1>&2
        exit 1
    fi
) || loud_fail "pi setup onboarding failed: $(tail -20 "$scratch/pi.log")"

pi_cfg="$proj_pi/.wg/config.toml"
[[ -f "$pi_cfg" ]] || loud_fail "expected repo-local pi config at $pi_cfg"
grep -q 'inherit_global = true' "$pi_cfg" \
    || loud_fail "pi local setup should reuse global OpenRouter login via inherit_global: $(cat "$pi_cfg")"
grep -q 'model = "pi:openrouter/z-ai/glm-5.2"' "$pi_cfg" \
    || loud_fail "pi setup should preserve the pi strong route: $(cat "$pi_cfg")"
if grep -q 'api_key_ref' "$pi_cfg"; then
    loud_fail "pi local setup should not copy the global OpenRouter secret ref into repo-local config"
fi

echo "PASS: setup onboarding stores OpenRouter secrets by ref and reuses global WG OpenRouter login for repo-local Pi setup"
exit 0
