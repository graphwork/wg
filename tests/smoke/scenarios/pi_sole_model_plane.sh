#!/usr/bin/env bash
# Smoke: supported configuration/discovery is Pi-only and exact unregistered
# Pi routes resolve with explicit reasoning without touching credentials.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME"
cd "$scratch"
WG_DIR_FLAG="--dir $scratch/.wg"
wg() { command wg $WG_DIR_FLAG "$@"; }

wg init --no-agency >/dev/null
route='pi:future-provider:vendor/model-not-in-wg'
wg setup --route pi --yes --scope local --model "$route" >/dev/null

[ ! -e "$scratch/.wg/models.yaml" ] \
    || loud_fail "Pi setup unexpectedly created a WG model registry"
! grep -q '\[\[model_registry\]\]' "$scratch/.wg/config.toml" \
    || loud_fail "Pi setup persisted a legacy model registry"
! grep -q '\[\[llm_endpoints' "$scratch/.wg/config.toml" \
    || loud_fail "Pi setup persisted a WG endpoint"
! grep -Eq '^[[:space:]]*executor[[:space:]]*=' "$scratch/.wg/config.toml" \
    || loud_fail "Pi setup persisted a legacy executor selector"

effective=$(wg config --models 2>&1) \
    || loud_fail "effective Pi role display failed: $effective"
grep -q 'Pi Model Plane' <<<"$effective" \
    || loud_fail "effective config did not identify Pi as model plane: $effective"
if grep -E '^[[:space:]]+[a-z_]+[[:space:]]+' <<<"$effective" \
    | grep -vE 'HANDLER|^[-=[:space:]]*$' \
    | grep -vq ' pi '; then
    loud_fail "an effective LLM role was not handler pi: $effective"
fi
role_count=$(grep -c ' pi ' <<<"$effective" || true)
[ "$role_count" -ge 14 ] \
    || loud_fail "not every role was rendered with handler pi: $effective"
grep -q "$route" <<<"$effective" \
    || loud_fail "unregistered exact Pi route was replaced/blocked: $effective"
grep -Eq ' (low|high|xhigh) ' <<<"$effective" \
    || loud_fail "effective reasoning was not visible: $effective"

# The real worker planner must accept the exact, unregistered route without
# consulting a WG model catalog. Dry-run stops before invoking Pi.
wg add 'Pi route probe' --id pi-route-probe -d 'credential-free planner probe' >/dev/null
spawn=$(WG_MODEL="$route" WG_EXECUTOR_TYPE=pi WG_REASONING=high \
    wg spawn-task --dry-run pi-route-probe 2>&1) \
    || loud_fail "unregistered Pi route did not dispatch: $spawn"
grep -q -- '--provider future-provider --model vendor/model-not-in-wg' <<<"$spawn" \
    || loud_fail "Pi dispatch identity was rewritten: $spawn"
! grep -Eq 'claude-handler|codex-handler|nex|native' <<<"$spawn" \
    || loud_fail "Pi dispatch fell back to another execution system: $spawn"

if wg config --local --model codex:gpt-x >/tmp/pi-plane-nonpi.out 2>&1; then
    loud_fail "non-Pi route was accepted"
fi
grep -q 'Pi' /tmp/pi-plane-nonpi.out \
    || loud_fail "non-Pi rejection was not explicit: $(cat /tmp/pi-plane-nonpi.out)"

quick=$(wg quickstart 2>&1)
grep -q 'Pi is the sole LLM model plane' <<<"$quick" \
    || loud_fail "quickstart did not name Pi ownership"
for retired in 'wg model list' 'wg models search' 'wg endpoints add' 'wg key set' 'setup --route claude-cli' 'setup --route codex-cli' 'setup --route nex-custom'; do
    ! grep -q -- "$retired" <<<"$quick" \
        || loud_fail "quickstart advertised retired model-plane choice: $retired"
done

# Removing all inherited reasoning must fail the effective-config boundary.
python3 - "$scratch/.wg/config.toml" <<'PY'
from pathlib import Path
p=Path(__import__('sys').argv[1])
s=p.read_text()
for line in [
    'fast_reasoning = "low"\n',
    'standard_reasoning = "high"\n',
    'premium_reasoning = "xhigh"\n',
    'reasoning = "high"\n',
    'reasoning = "low"\n',
]:
    s=s.replace(line, '')
p.write_text(s)
PY
if wg config --models >/tmp/pi-plane-reasoning.out 2>&1; then
    loud_fail "missing reasoning did not fail closed"
fi
grep -q 'WG-PI-REASONING-MISSING' /tmp/pi-plane-reasoning.out \
    || loud_fail "missing reasoning error was not stable: $(cat /tmp/pi-plane-reasoning.out)"

echo "PASS: Pi is the sole model configuration plane"
