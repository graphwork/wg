#!/usr/bin/env bash
# Smoke: active named codex profile applies to the real service-start flow.
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

if ! wg init -m claude:opus --no-agency >init.log 2>&1; then
    loud_fail "wg init with local claude:opus config failed:
$(cat init.log)"
fi

if ! grep -q 'model = "claude:opus"' .wg/config.toml; then
    loud_fail "fixture did not start with a local claude:opus pin:
$(cat .wg/config.toml)"
fi

# Mirror the user flow: stop service, select codex, start service.
wg --dir "$scratch/.wg" service stop --force --kill-agents >/dev/null 2>&1 || true

if ! wg --dir "$scratch/.wg" profile use codex --no-reload >profile-use.log 2>&1; then
    loud_fail "wg profile use codex failed:
$(cat profile-use.log)"
fi

profile_show=$(wg --dir "$scratch/.wg" profile show 2>&1) || \
    loud_fail "wg profile show failed"
if ! grep -q 'Active named profile: codex' <<<"$profile_show"; then
    loud_fail "wg profile show did not report active codex profile:
$profile_show"
fi
if ! grep -q 'agent.model  = codex:gpt-5.5' <<<"$profile_show"; then
    loud_fail "wg profile show did not apply codex agent model over local claude pin:
$profile_show"
fi
if ! grep -q 'dispatcher.model = codex:gpt-5.5' <<<"$profile_show"; then
    loud_fail "wg profile show did not apply codex dispatcher model over local claude pin:
$profile_show"
fi

start_wg_daemon "$scratch" --max-agents 0 --no-coordinator-agent --interval 60

wrapper_log="$scratch/daemon.log"
daemon_log="$WG_SMOKE_DAEMON_DIR/service/daemon.log"

if ! grep -qE 'Dispatcher: .*executor=codex, model=codex:gpt-5\.5' "$wrapper_log"; then
    loud_fail "wg service start did not print codex dispatcher settings:
$(cat "$wrapper_log" 2>/dev/null || true)"
fi

if ! grep -qE 'Coordinator config: .*executor=codex, model=codex:gpt-5\.5' "$daemon_log"; then
    loud_fail "daemon did not boot with codex dispatcher settings:
$(cat "$daemon_log" 2>/dev/null || true)"
fi

models_out=$(wg --dir "$scratch/.wg" config --models 2>&1) || \
    loud_fail "wg config --models failed"
if grep -qE 'evaluator[[:space:]]+fast[[:space:]]+gpt-5\.4-mini[[:space:]]+nex' <<<"$models_out"; then
    loud_fail "wg config --models rendered codex evaluator as nex:
$models_out"
fi
if ! grep -qE 'evaluator[[:space:]]+fast[[:space:]]+gpt-5\.4-mini[[:space:]]+codex' <<<"$models_out"; then
    loud_fail "wg config --models did not render codex evaluator provider:
$models_out"
fi

echo "PASS: active codex profile overrides local claude pins for profile show, service start, and model routing display"
