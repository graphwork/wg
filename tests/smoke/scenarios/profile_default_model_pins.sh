#!/usr/bin/env bash
# Smoke: profile/default model pins stay on the top worker defaults.
set -u
source "$(dirname "$0")/_helpers.sh"
require_wg

scratch=$(make_scratch)
export HOME="$scratch/home"
export XDG_CONFIG_HOME="$HOME/.config"
mkdir -p "$HOME/.wg" "$XDG_CONFIG_HOME" "$scratch/proj/.wg"
wg_dir="$scratch/proj/.wg"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" wg --dir "$wg_dir" "$@"
}

assert_route_file() {
    local path="$1"
    local route="$2"
    python3 - "$path" "$route" <<'PY'
import sys
import tomllib

path, route = sys.argv[1], sys.argv[2]
with open(path, "rb") as f:
    data = tomllib.load(f)

def get(dotted):
    cur = data
    for part in dotted.split("."):
        cur = cur[part]
    return cur

if route == "codex":
    top = "codex:gpt-5.5"
    cheap = "codex:gpt-5.4-mini"
    stale = {"codex:gpt-5.4", "gpt-5.4"}
elif route == "claude":
    top = "claude:opus"
    cheap = "claude:haiku"
    stale = {"claude:sonnet", "sonnet"}
else:
    raise AssertionError(route)

top_paths = [
    "agent.model",
    "dispatcher.model",
    "tiers.standard",
    "tiers.premium",
    "models.default.model",
    "models.task_agent.model",
]
for dotted in top_paths:
    actual = get(dotted)
    if actual != top:
        raise AssertionError(f"{path}: {dotted} = {actual!r}, want {top!r}")
    if actual in stale:
        raise AssertionError(f"{path}: stale default pin at {dotted}: {actual!r}")

for dotted in [
    "models.evaluator.model",
    "models.assigner.model",
    "models.flip_inference.model",
    "models.flip_comparison.model",
]:
    actual = get(dotted)
    if actual != cheap:
        raise AssertionError(f"{path}: {dotted} = {actual!r}, want cheap agency pin {cheap!r}")
PY
}

assert_models_json() {
    local expected_provider="$1"
    local expected_model="$2"
    local json="$3"
    MODELS_JSON="$json" python3 - "$expected_provider" "$expected_model" <<'PY'
import json
import os
import sys

provider, model = sys.argv[1], sys.argv[2]
data = json.loads(os.environ["MODELS_JSON"])
for role in ["default", "task_agent"]:
    actual_model = data[role]["model"]
    actual_provider = data[role]["provider"]
    if actual_model != model or actual_provider != provider:
        raise AssertionError(
            f"{role}: got provider={actual_provider!r} model={actual_model!r}, "
            f"want provider={provider!r} model={model!r}"
        )
PY
}

assert_standard_tier_json() {
    local expected="$1"
    local json="$2"
    TIERS_JSON="$json" python3 - "$expected" <<'PY'
import json
import os
import sys

expected = sys.argv[1]
data = json.loads(os.environ["TIERS_JSON"])
actual = data["standard"]["model_id"]
if actual != expected:
    raise AssertionError(f"standard tier = {actual!r}, want {expected!r}")
PY
}

run_wg profile init-starters >/tmp/profile-pins-init.log 2>&1 || \
    loud_fail "wg profile init-starters failed: $(cat /tmp/profile-pins-init.log)"

assert_route_file "$HOME/.wg/profiles/codex.toml" codex
assert_route_file "$HOME/.wg/profiles/claude.toml" claude

run_wg profile use codex --no-reload >/tmp/profile-pins-use-codex.log 2>&1 || \
    loud_fail "wg profile use codex failed: $(cat /tmp/profile-pins-use-codex.log)"
models_json=$(run_wg --json config --models 2>&1) || \
    loud_fail "wg config --models --json failed for codex: $models_json"
assert_models_json codex gpt-5.5 "$models_json"
tiers_json=$(run_wg --json config --tiers 2>&1) || \
    loud_fail "wg config --tiers --json failed for codex: $tiers_json"
assert_standard_tier_json codex:gpt-5.5 "$tiers_json"

run_wg profile use claude --no-reload >/tmp/profile-pins-use-claude.log 2>&1 || \
    loud_fail "wg profile use claude failed: $(cat /tmp/profile-pins-use-claude.log)"
models_json=$(run_wg --json config --models 2>&1) || \
    loud_fail "wg config --models --json failed for claude: $models_json"
assert_models_json anthropic opus "$models_json"
tiers_json=$(run_wg --json config --tiers 2>&1) || \
    loud_fail "wg config --tiers --json failed for claude: $tiers_json"
assert_standard_tier_json claude:opus "$tiers_json"

check_setup_route() {
    local route="$1"
    local profile="$2"
    local route_home="$scratch/setup-$route-home"
    mkdir -p "$route_home/.wg"
    HOME="$route_home" XDG_CONFIG_HOME="$route_home/.config" \
        wg setup --route "$route" --yes >/tmp/profile-pins-setup-$route.log 2>&1 || \
        loud_fail "wg setup --route $route --yes failed: $(cat /tmp/profile-pins-setup-$route.log)"
    assert_route_file "$route_home/.wg/config.toml" "$profile"
}

check_config_init_route() {
    local route="$1"
    local profile="$2"
    local route_home="$scratch/init-$route-home"
    local route_wg="$scratch/init-$route-proj/.wg"
    mkdir -p "$route_home/.wg" "$route_wg"
    HOME="$route_home" XDG_CONFIG_HOME="$route_home/.config" \
        wg --dir "$route_wg" config init --global --route "$route" --force \
        >/tmp/profile-pins-config-init-$route.log 2>&1 || \
        loud_fail "wg config init --route $route failed: $(cat /tmp/profile-pins-config-init-$route.log)"
    assert_route_file "$route_home/.wg/config.toml" "$profile"
}

check_setup_route codex-cli codex
check_setup_route claude-cli claude
check_config_init_route codex-cli codex
check_config_init_route claude-cli claude

python3 - "$HOME/.wg/profiles/codex.toml" "$HOME/.wg/profiles/claude.toml" <<'PY'
import sys
for path, old, new in [
    (sys.argv[1], 'standard = "codex:gpt-5.5"', 'standard = "codex:gpt-5.4"'),
    (sys.argv[2], 'standard = "claude:opus"', 'standard = "claude:sonnet"'),
]:
    body = open(path, encoding="utf-8").read()
    if old not in body:
        raise SystemExit(f"{old!r} not found in {path}")
    open(path, "w", encoding="utf-8").write(body.replace(old, new, 1))
PY

qualified_out=$(run_wg profile use codex:gpt-5.5 --no-reload 2>&1) || \
    loud_fail "wg profile use codex:gpt-5.5 failed: $qualified_out"
if ! grep -q "Default/task-agent route pinned to codex:gpt-5.5" <<<"$qualified_out"; then
    loud_fail "qualified codex activation did not report exact pin: $qualified_out"
fi
profile_show=$(run_wg profile show 2>&1) || \
    loud_fail "wg profile show failed after codex:gpt-5.5: $profile_show"
for expected in \
    "Active named profile: codex" \
    "models.default   = codex:gpt-5.5" \
    "models.task_agent= codex:gpt-5.5" \
    "standard = codex:gpt-5.5"; do
    if ! grep -q "$expected" <<<"$profile_show"; then
        loud_fail "profile show missing '$expected' after codex:gpt-5.5. Got:\n$profile_show"
    fi
done
if grep -q "standard = codex:gpt-5.4" <<<"$profile_show"; then
    loud_fail "profile show still exposes stale codex standard pin as authoritative:\n$profile_show"
fi

qualified_out=$(run_wg profile use claude:opus --no-reload 2>&1) || \
    loud_fail "wg profile use claude:opus failed: $qualified_out"
if ! grep -q "Default/task-agent route pinned to claude:opus" <<<"$qualified_out"; then
    loud_fail "qualified claude activation did not report exact pin: $qualified_out"
fi
profile_show=$(run_wg profile show 2>&1) || \
    loud_fail "wg profile show failed after claude:opus: $profile_show"
for expected in \
    "Active named profile: claude" \
    "models.default   = claude:opus" \
    "models.task_agent= claude:opus" \
    "standard = claude:opus"; do
    if ! grep -q "$expected" <<<"$profile_show"; then
        loud_fail "profile show missing '$expected' after claude:opus. Got:\n$profile_show"
    fi
done
if grep -q "standard = claude:sonnet" <<<"$profile_show"; then
    loud_fail "profile show still exposes stale claude standard pin as authoritative:\n$profile_show"
fi

run_wg profile use codex --no-reload >/tmp/profile-pins-active-codex.log 2>&1 || \
    loud_fail "wg profile use codex before direct edit failed: $(cat /tmp/profile-pins-active-codex.log)"
direct_out=$(run_wg config --global --model claude:opus --no-reload 2>&1) || \
    loud_fail "wg config --global --model claude:opus failed: $direct_out"
if ! grep -q "Active profile cleared" <<<"$direct_out"; then
    loud_fail "direct global model edit did not report active profile clear: $direct_out"
fi
if [[ -f "$HOME/.wg/active-profile" ]]; then
    loud_fail "active-profile pointer still exists after direct global model edit"
fi
assert_route_file "$HOME/.wg/config.toml" claude

run_wg profile use codex --no-reload >/tmp/profile-pins-override-codex.log 2>&1 || \
    loud_fail "wg profile use codex before local override failed: $(cat /tmp/profile-pins-override-codex.log)"
cat >"$wg_dir/config.toml" <<'EOF'
[agent]
model = "openrouter:custom/project-model"

[dispatcher]
model = "openrouter:custom/project-model"

[tiers]
standard = "openrouter:custom/project-model"
premium = "openrouter:custom/project-model"

[models.default]
model = "openrouter:custom/project-model"

[models.task_agent]
model = "openrouter:custom/project-model"
EOF
models_json=$(run_wg --json config --models 2>&1) || \
    loud_fail "wg config --models --json failed for project-local override: $models_json"
assert_models_json openrouter custom/project-model "$models_json"

echo "PASS: profile default model pins resolve codex:gpt-5.5 and claude:opus across fresh profiles, setup, config init, active-profile edits, and local overrides"
