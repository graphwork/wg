#!/usr/bin/env bash
# Smoke: profile use is authoritative over local routing pins and hot-reloads a running daemon.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME/.wg"
cd "$scratch"

if ! wg profile init-starters >profile-init.log 2>&1; then
    loud_fail "wg profile init-starters failed:
$(cat profile-init.log)"
fi

if ! wg init --no-agency >init.log 2>&1; then
    loud_fail "wg init failed:
$(cat init.log)"
fi

cat >.wg/config.toml <<'TOML'
[agent]
model = "claude:opus"
executor = "claude"

[dispatcher]
model = "claude:opus"
executor = "claude"
max_agents = 4

[tiers]
fast = "claude:haiku"
standard = "claude:sonnet"
premium = "claude:opus"

[models.default]
model = "claude:opus"

[models.evaluator]
model = "claude:haiku"

[agency]
auto_assign = false
auto_evaluate = false
assigner_agent = "local-assigner"
TOML

start_wg_daemon "$scratch" --max-agents 0 --no-coordinator-agent --interval 60

before_status=$(wg --dir "$scratch/.wg" service status 2>&1) || \
    loud_fail "initial wg service status failed:
$before_status"
if ! grep -qE 'Dispatcher: .*executor=claude, model=claude:opus' <<<"$before_status"; then
    loud_fail "fixture daemon did not start on the local claude pin:
$before_status"
fi

profile_use=$(wg --dir "$scratch/.wg" profile use codex 2>&1) || \
    loud_fail "wg profile use codex failed:
$profile_use"
if ! grep -q 'Cleared local routing overrides' <<<"$profile_use"; then
    loud_fail "profile use codex did not report clearing stale local routing pins:
$profile_use"
fi
if ! grep -q 'Daemon reloaded' <<<"$profile_use"; then
    loud_fail "profile use codex did not hot-reload the running daemon:
$profile_use"
fi

profile_show=$(wg --dir "$scratch/.wg" profile show 2>&1) || \
    loud_fail "wg profile show failed:
$profile_show"
if ! grep -q 'Active named profile: codex' <<<"$profile_show"; then
    loud_fail "profile show did not report active codex profile:
$profile_show"
fi
if ! grep -q 'agent.model.*codex:gpt-5\.5' <<<"$profile_show"; then
    loud_fail "profile show did not report codex agent model:
$profile_show"
fi
if grep -q 'claude:opus' <<<"$profile_show"; then
    loud_fail "profile show still exposed stale local claude model after codex switch:
$profile_show"
fi

merged=$(wg --dir "$scratch/.wg" config --merged 2>&1) || \
    loud_fail "wg config --merged failed:
$merged"
if ! grep -q 'executor = "codex"' <<<"$merged"; then
    loud_fail "merged config did not resolve codex executor:
$merged"
fi
if ! grep -q 'model = "codex:gpt-5\.5"' <<<"$merged"; then
    loud_fail "merged config did not resolve codex model:
$merged"
fi
if ! grep -q 'max_agents = 4' <<<"$merged"; then
    loud_fail "merged config did not preserve local dispatcher.max_agents:
$merged"
fi
if ! grep -q 'assigner_agent = "local-assigner"' <<<"$merged"; then
    loud_fail "merged config did not preserve unrelated agency setting:
$merged"
fi

after_status=$(wg --dir "$scratch/.wg" service status 2>&1) || \
    loud_fail "wg service status after codex switch failed:
$after_status"
if ! grep -qE 'Dispatcher: .*executor=codex, model=codex:gpt-5\.5' <<<"$after_status"; then
    loud_fail "running daemon did not report codex executor/model after hot reload:
$after_status"
fi

local_cfg=$(cat .wg/config.toml)
if ! grep -q 'max_agents = 4' <<<"$local_cfg" || \
   ! grep -q 'assigner_agent = "local-assigner"' <<<"$local_cfg"; then
    loud_fail "local config did not preserve unrelated settings:
$local_cfg"
fi
if grep -qE 'claude:(opus|haiku)|^\[tiers\]|^\[models' <<<"$local_cfg"; then
    loud_fail "local config still contains stale model routing pins:
$local_cfg"
fi

switch_back=$(wg --dir "$scratch/.wg" profile use claude 2>&1) || \
    loud_fail "wg profile use claude failed:
$switch_back"
if ! grep -q 'Daemon reloaded' <<<"$switch_back"; then
    loud_fail "profile use claude did not hot-reload the running daemon:
$switch_back"
fi

back_status=$(wg --dir "$scratch/.wg" service status 2>&1) || \
    loud_fail "wg service status after claude switch failed:
$back_status"
if ! grep -qE 'Dispatcher: .*executor=claude, model=claude:opus' <<<"$back_status"; then
    loud_fail "running daemon did not report claude executor/model after switching back:
$back_status"
fi

echo "PASS: profile_switch_authoritative_hot_reload"
