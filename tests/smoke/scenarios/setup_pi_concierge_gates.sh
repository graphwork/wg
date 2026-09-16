#!/usr/bin/env bash
# Scenario: setup_pi_concierge_gates
#
# Pins the concierge Pi-readiness gates in front of attended interactive
# `wg setup` (concierge-guided-pi). WG is embedded in the Pi model plane, so
# the attended wizard must walk a brand-new user through Pi readiness FIRST,
# in three fail-clean gates, before any prompt and before any byte is
# written:
#
#   Gate 1 — PI DETECTED?      no `pi` on PATH ⇒ print the exact install
#                              command (`npm install -g
#                              @earendil-works/pi-coding-agent`, Node 20+)
#                              and exit cleanly with the rerun instruction.
#   Gate 2 — AUTHENTICATED?    `pi` present but no provider authenticated ⇒
#                              print the guided `/login <provider>`
#                              instruction (Pi owns the OAuth flow; WG never
#                              sees keys) and exit cleanly.
#   Gate 3 — MODEL RESOLVABLE? authenticated but no model resolves ⇒ stop
#                              cleanly (exercised here via the sanctioned
#                              injectable-probes bridge).
#
# The gates sit between the route selection and the model wizard: a user who
# explicitly declines execution ("Not now — keep this WG graph-only") never
# reaches them; a user continuing into the strong/weak tier prompts must
# clear all three first.
#
# Every stop must be FAIL-CLEAN: exit 0 (a clean "come back when ready"
# return, not an error), NO worksgood.toml written, and the strong/weak
# tier prompts never reached.
#
# Harness bridge: the live probes are injectable via WORKSGOOD_PI_GATE_JSON
# (the same convention as WORKSGOOD_PI_MODELS_JSON). Gate 1 and gate 2 are
# exercised through the fake-pi fixture (a fake `pi` on PATH / an empty
# PATH); gate 3 and the all-green pass-through use the injection bridge so
# the scenario stays credential-free and hermetic on any host.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

WG_BIN="${WG_BIN:-$(command -v wg)}"

scratch=$(make_scratch)

fake_home="$scratch/home"
mkdir -p "$fake_home" "$scratch/global"
export HOME="$fake_home"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_AGENT_ROLE WG_EXECUTOR_TYPE WG_MODEL WG_TIER 2>/dev/null || true

scenario_step() {
    printf 'SMOKE STEP %s: %s\n' "$1" "$2" >&2
}

GREEN_PROBES='{"pi_present":true,"providers":["openrouter"],"authenticated":["openrouter"],"models":["pi:openrouter:z-ai/glm-5.2"]}'

run_wizard() {
    # $1 = project dir, $2 = inner PATH, $3 = typescript out, stdin = keystrokes
    local proj="$1" inner_path="$2" ts="$3"
    timeout 60s script -qec \
        "cd '$proj' && env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER HOME='$fake_home' WG_GLOBAL_DIR='$scratch/global' PATH='$inner_path' '$WG_BIN' --dir '$proj/.wg' setup" \
        "$ts"
}

# The gates sit after the route picker, so a stop run accepts the default Pi
# route with one Enter and must get no further than the gate message.
STOP_KEYSTROKES=$'\n'

# ── fake-pi fixture ──────────────────────────────────────────────────
# Emulates just enough of `pi` for the gate probes:
#   * `--mode rpc --no-session -ne` → the bounded offline catalog reply
#     (models with their providers),
#   * `auth check --provider <P> --json --no-refresh` → not_ready (exit 1),
#     unless FAKE_PI_AUTH=ready.
# It drains stdin before replying so the parent's request write can never
# race into EPIPE, and logs every invocation for the no-side-effect asserts.
make_fake_pi() {
    local bin="$1"
    mkdir -p "$bin"
    cat >"$bin/pi" <<'FAKEPI'
#!/bin/bash
printf 'FAKEPI %s\n' "$*" >>"__FAKE_PI_LOG__"
case " $* " in
    *" --mode rpc "*)
        # Drain the parent's request first (no EPIPE race), then reply with
        # the offline catalog: two models, one provider.
        while IFS= read -r line; do :; done
        echo '{"id":"worksgood-catalog","success":true,"data":{"models":[{"provider":"openrouter","id":"z-ai/glm-5.2","name":"GLM","reasoning":true},{"provider":"openrouter","id":"deepseek/deepseek-chat","name":"DS","reasoning":true}]}}'
        exit 0
        ;;
esac
case " $* " in
    *" auth check "*)
        prev=""; p=""
        for a in "$@"; do
            [[ "$prev" == "--provider" ]] && p="$a"
            prev="$a"
        done
        if [[ "${FAKE_PI_AUTH:-not_ready}" == "ready" ]]; then
            echo "{\"status\":\"ready\",\"provider\":\"$p\",\"authType\":\"api_key\"}"
            exit 0
        fi
        echo "{\"status\":\"not_ready\",\"provider\":\"$p\",\"reason\":\"no_credentials\"}"
        exit 1
        ;;
esac
echo "fake-pi: unexpected invocation: $*" >>"${FAKE_PI_LOG:?}"
exit 1
FAKEPI
    # The log path is substituted at fixture time: the wizard's inner env is
    # PATH-only, so the fake pi cannot rely on an inherited variable.
    sed -i "s|__FAKE_PI_LOG__|$FAKE_PI_LOG|" "$bin/pi"
    chmod +x "$bin/pi"
}

assert_clean_stop() {
    # $1 = typescript, $2 = project dir, $3 = expected marker greps (regex list)
    local ts="$1" proj="$2"
    shift 2
    local marker
    for marker in "$@"; do
        grep -aqE "$marker" "$ts" ||
            loud_fail "gate output missing expected marker '$marker':\n$(tail -30 "$ts" | tr -d '\r')"
    done
    # Fail-clean: exit 0 is asserted by the caller; the route prompt was
    # reached (Enter accepted the default Pi route), but the gate must stop
    # BEFORE the model wizard, and NOTHING may be written.
    grep -aq 'Pick a route' "$ts" ||
        loud_fail "gate output missing the route prompt; the gate harness drifted:\n$(tail -30 "$ts" | tr -d '\r')"
    if grep -aq 'STRONG route' "$ts"; then
        loud_fail "gate stopped too late: the wizard reached the STRONG route prompt:\n$(tail -30 "$ts" | tr -d '\r')"
    fi
    if [[ -e "$proj/worksgood.toml" ]]; then
        loud_fail "gate was NOT fail-clean: worksgood.toml was written:\n$(cat "$proj/worksgood.toml")"
    fi
}

# ── STEP 1: gate 1 — pi missing ─────────────────────────────────────
scenario_step 1 'pi missing on PATH: exact install command + clean exit, no partial state'

proj1="$scratch/project-pi-missing"
mkdir -p "$proj1/.wg"
empty_bin="$scratch/empty-bin"
mkdir -p "$empty_bin"

# The gate runs after the route picker, so the single Enter accepts the
# default Pi route and the gate stops before the model wizard.
if ! printf '%s' "$STOP_KEYSTROKES" | run_wizard "$proj1" "$empty_bin" "$scratch/gate1.typescript"; then
    loud_fail "pi-missing gate did not exit cleanly (exit 0 expected):\n$(tail -30 "$scratch/gate1.typescript" | tr -d '\r')"
fi
assert_clean_stop "$scratch/gate1.typescript" "$proj1" \
    'npm install -g @earendil-works/pi-coding-agent' \
    'Node 20\+' \
    'Rerun `worksgood setup` after installing Pi' \
    'No configuration was written'

# ── STEP 2: gate 2 — pi present, no provider authenticated ──────────
scenario_step 2 'pi present + no auth: guided /login instruction + clean exit, no partial state'

proj2="$scratch/project-no-auth"
mkdir -p "$proj2/.wg"
fake_bin="$scratch/fake-bin"
FAKE_PI_LOG="$scratch/fake-pi.log" make_fake_pi "$fake_bin"

if ! printf '%s' "$STOP_KEYSTROKES" | run_wizard "$proj2" "$fake_bin" "$scratch/gate2.typescript"; then
    loud_fail "no-auth gate did not exit cleanly (exit 0 expected):\n$(tail -30 "$scratch/gate2.typescript" | tr -d '\r')"
fi
assert_clean_stop "$scratch/gate2.typescript" "$proj2" \
    'no provider is authenticated' \
    '/login <provider>' \
    '/login openrouter' \
    'Pi owns the OAuth flow' \
    'WG never sees or stores your keys' \
    'Rerun `worksgood setup` after logging in' \
    'No configuration was written'
# The gate must have actually probed Pi's own readiness check (auth check),
# never a WG-side credential flow.
grep -q 'auth check' "$scratch/fake-pi.log" ||
    loud_fail "gate 2 did not run Pi's own credential readiness check:\n$(cat "$scratch/fake-pi.log")"
# The catalog probe (Pi's offline registry) must have been used to derive
# the exposed providers.
grep -q -- '--mode rpc' "$scratch/fake-pi.log" ||
    loud_fail "gate probes did not consult Pi's offline catalog:\n$(cat "$scratch/fake-pi.log")"

# ── STEP 3: gate 3 — authenticated but no resolvable model (injected) ──
scenario_step 3 'authenticated + no resolvable model: clean stop via the injectable-probes bridge'

proj3="$scratch/project-no-model"
mkdir -p "$proj3/.wg"
export WORKSGOOD_PI_GATE_JSON='{"pi_present":true,"providers":["openrouter"],"authenticated":["openrouter"],"models":[]}'
if ! printf '%s' "$STOP_KEYSTROKES" | run_wizard "$proj3" "$fake_bin" "$scratch/gate3.typescript"; then
    loud_fail "no-model gate did not exit cleanly (exit 0 expected):\n$(tail -30 "$scratch/gate3.typescript" | tr -d '\r')"
fi
unset WORKSGOOD_PI_GATE_JSON
assert_clean_stop "$scratch/gate3.typescript" "$proj3" \
    'no model resolves' \
    'pi --list-models' \
    'Rerun `worksgood setup` once at least one model resolves' \
    'No configuration was written'

# ── STEP 4: all green → the wizard proceeds past the gates ──────────
scenario_step 4 'all green (injected): gates pass through and the wizard reaches its prompts'

proj4="$scratch/project-green"
mkdir -p "$proj4/.wg"
export WORKSGOOD_PI_GATE_JSON="$GREEN_PROBES"
# Route default (Pi) — gates pass — STRONG default, WEAK Enter (reuse
# strong), agency, max-agents, write-confirm, notify skip — the same
# keystroke contract as setup_two_tier_routes.sh.
if ! printf '\n\n\n\n\n\n\n' | run_wizard "$proj4" "$fake_bin" "$scratch/green.typescript"; then
    loud_fail "all-green setup did not complete:\n$(tail -40 "$scratch/green.typescript" | tr -d '\r')"
fi
unset WORKSGOOD_PI_GATE_JSON
grep -aq 'Pi readiness: OK' "$scratch/green.typescript" ||
    loud_fail "all-green run did not print the readiness summary:\n$(head -30 "$scratch/green.typescript" | tr -d '\r')"
grep -aq 'Pick a route' "$scratch/green.typescript" ||
    loud_fail "all-green run never reached the wizard prompts:\n$(tail -40 "$scratch/green.typescript" | tr -d '\r')"
[[ -f "$proj4/worksgood.toml" ]] || loud_fail "all-green setup wrote no worksgood.toml"
grep -qE '^model = "pi:openrouter:z-ai/glm-5.2"' "$proj4/worksgood.toml" ||
    loud_fail "all-green setup wrote an unexpected route:\n$(cat "$proj4/worksgood.toml")"

echo "PASS: concierge Pi-readiness gates stop fail-clean (pi-missing, no-auth, no-model) and pass through when green"
