#!/usr/bin/env bash
# Scenario: dispatch_decoupled_from_openrouter_registry
#
# Regression lock for fix-decouple-dispatch
# (wg-bug-openrouter-model-resolution): a model spec whose leading token is a
# handler/executor that resolves the model itself (e.g. `pi:zai:glm-5.2`,
# which the `pi` handler reaches natively) MUST NOT wedge the dispatcher when
# the OpenRouter catalog refresh is unavailable. Concretely, with an EMPTY
# model registry and NO OpenRouter API key:
#
#   * `wg config --show` (the config-load validation surface) must NOT emit an
#     `unresolved-model-id` warning for the executor-owned model — the
#     pre-fix code warned and then the daemon's OpenRouter refresh failed into
#     a 60-min cooldown that looked like a wedged dispatcher.
#   * A declared `[[model_registry]]` entry must resolve the spec with no
#     network (no warning either).
#   * A genuinely unresolved bare alias must STILL warn (the safety signal is
#     retained — only fail-soft for executor-owned/declared models).
#
# Credential-free: no LLM call, no OpenRouter key, no daemon spawn. Isolates
# HOME so the host's real global ~/.wg/config.toml cannot leak in.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
# MUST cd into the scratch so the workgraph-dir walk-up resolves
# $scratch/.wg, not this repo's real .wg (which carries a profile association
# that would mask the regression under test).
cd "$scratch"
home="$scratch/home"
mkdir -p "$home"

# Isolate the WG global dir so Config::load_merged cannot pick up this
# machine's real global config or active-profile (which may carry an
# OpenRouter endpoint/key and mask the regression). Also ensure no
# OpenRouter key leaks from the environment.
export WG_GLOBAL_DIR="$home/.wg"
mkdir -p "$WG_GLOBAL_DIR"
unset OPENROUTER_API_KEY || true
unset OPENAI_API_KEY || true

workdir="$scratch/.wg"
mkdir -p "$workdir"

run_config_show() {
    # `wg config --show` loads merged config and prints a [health check]
    # section that includes validate_config() warnings (rule + message).
    wg config --show 2>&1
}

# ── Test 1: executor-owned model + empty registry + no key → NO warning. ──
cat >"$workdir/config.toml" <<'TOML'
[coordinator]
registry_refresh_interval = 0
[agent]
model = "pi:zai:glm-5.2"
[models.default]
model = "pi:zai:glm-5.2"
TOML

out=$(run_config_show) || loud_fail "wg config --show failed for executor-owned model:\n$out"

if echo "$out" | grep -q "unresolved-model-id"; then
    loud_fail "executor-owned pi:zai:glm-5.2 must NOT trigger unresolved-model-id:\n$out"
fi
if echo "$out" | grep -q "doesn't match any registry entry"; then
    loud_fail "executor-owned pi:zai:glm-5.2 must not warn about registry mismatch:\n$out"
fi

# ── Test 2: a [[model_registry]] entry resolves the spec with no network. ──
cat >"$workdir/config.toml" <<'TOML'
[coordinator]
registry_refresh_interval = 60
[agent]
model = "pi:zai:glm-5.2"
[models.default]
model = "pi:zai:glm-5.2"
[[model_registry]]
id = "glm-5.2"
provider = "zai"
model = "glm-5.2"
tier = "standard"
TOML

out=$(run_config_show) || loud_fail "wg config --show failed for declared registry entry:\n$out"
if echo "$out" | grep -q "unresolved-model-id"; then
    loud_fail "declared [[model_registry]] entry must resolve without a warning:\n$out"
fi
if echo "$out" | grep -q "registry-model-format"; then
    loud_fail "non-OpenRouter provider (zai) must not be nagged for a bare model name:\n$out"
fi

# ── Test 3: genuinely unresolved spec STILL warns (safety retained). ──
# Use a bare provider-prefixed spec (passes strict parse, is NOT a handler so
# the executor does not own it, and carries no '/' so it is not a vendor/model
# path) — exactly the shape that should remain an unresolved-model-id warning.
cat >"$workdir/config.toml" <<'TOML'
[coordinator]
registry_refresh_interval = 0
[agent]
model = "openrouter:nonexistent-mystery-model"
[models.default]
model = "openrouter:nonexistent-mystery-model"
TOML

# `wg config --show` emits the bare-provider deprecation to stderr but must
# still exit 0 and surface the unresolved-model warning in the health check.
out=$(run_config_show) || loud_fail "wg config --show failed for unresolved spec:\n$out"
if ! echo "$out" | grep -q "doesn't match any registry entry"; then
    loud_fail "genuinely unresolved spec MUST still warn (safety signal retained):\n$out"
fi

echo "dispatch_decoupled_from_openrouter_registry: PASS"
exit 0
