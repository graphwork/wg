#!/usr/bin/env bash
# Scenario: concierge_operator_framing
#
# Pins the operator-framing contract of the attended `worksgood` lifecycle:
# the system must read as the operator's own tool, not a research artifact.
# Before the role plan is printed, run_lifecycle prints a plain (no emoji, no
# internal codenames) framing block:
#
#   * you are the operator; the graph is your durable record of work,
#   * the strong tier drives workers and heavy generative roles, the weak
#     tier drives the cheap recoverable one-shots (same-as-strong is valid),
#   * routes are choices, not commitments — re-changeable any time via
#     `wg config` / `wg profile select` — and Pi owns authentication and
#     model selection.
#
# The block must appear in the ATTENDED SETUP OUTPUT when the lifecycle
# reaches the plan stage, i.e. before "Immutable redacted plan:" in the
# dry-run plan flow (the same fixtures/conventions as the one-model setup
# scenario: fake `pi` on PATH, isolated HOME, `--dry-run` writes nothing).

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

WG_BIN="${WG_BIN:-$(command -v wg)}"

# The concierge binary ships in the same Cargo bundle as the wg candidate.
WORKSGOOD_BIN="$(dirname "$WG_BIN")/worksgood"
if [[ ! -x "$WORKSGOOD_BIN" ]]; then
    WORKSGOOD_BIN="$(command -v worksgood || true)"
fi
if [[ -z "$WORKSGOOD_BIN" || ! -x "$WORKSGOOD_BIN" ]]; then
    loud_skip "MISSING WORKSGOOD BINARY" "worksgood not found next to $WG_BIN or on PATH"
fi

scratch=$(make_scratch)

fake_home="$scratch/home"
mkdir -p "$fake_home" "$scratch/fake-bin"
export HOME="$fake_home"
export WG_GLOBAL_DIR="$fake_home/.wg"
export XDG_CACHE_HOME="$fake_home/.cache"
export XDG_CONFIG_HOME="$fake_home/.config"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_AGENT_ROLE WG_EXECUTOR_TYPE WG_MODEL WG_TIER 2>/dev/null || true

# Discovery/readiness fixture: just enough `pi` for the lifecycle to see an
# available Pi transport. The setup daemon makes no model call.
cat >"$scratch/fake-bin/pi" <<'FAKEPI'
#!/bin/sh
exit 0
FAKEPI
chmod +x "$scratch/fake-bin/pi"
export PATH="$scratch/fake-bin:$PATH"

proj="$scratch/project"
mkdir -p "$proj"
git -C "$proj" init -q 2>/dev/null || mkdir -p "$proj/.git"

M='pi:openrouter:deepseek/deepseek-v4-flash'

# Dry-run: the lifecycle reaches the plan stage without writing anything.
out="$scratch/plan.out"
if ! env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE \
        -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER \
        HOME="$fake_home" WG_GLOBAL_DIR="$fake_home/.wg" \
        "$WORKSGOOD_BIN" --project "$proj" setup --model "$M" --dry-run >"$out" 2>&1; then
    loud_fail "worksgood setup --model dry-run failed: $(tail -20 "$out")"
fi

# Dry-run must remain non-mutating.
[[ ! -e "$proj/.wg" && ! -e "$WG_GLOBAL_DIR" ]] \
    || loud_fail "dry-run wrote graph or global state"

# The framing block prints, in full, BEFORE the role plan.
plan_line="$(grep -n 'Immutable redacted plan:' "$out" | head -1 | cut -d: -f1)"
[[ -n "$plan_line" ]] || loud_fail "dry-run output never reached the plan stage: $(cat "$out")"
assert_before_plan() {
    local needle="$1" at
    at="$(grep -n -F "$needle" "$out" | head -1 | cut -d: -f1)"
    [[ -n "$at" ]] || loud_fail "framing line missing from attended setup output: $needle\n$(cat "$out")"
    (( at < plan_line )) || loud_fail "framing line printed after the plan: $needle"
}
assert_before_plan "You are the operator: this system is yours, not a research artifact."
assert_before_plan "The graph is your durable record of the work done here."
assert_before_plan "strong tier runs your workers and heavy generative roles"
assert_before_plan "weak tier runs the cheap, recoverable one-shots"
assert_before_plan "Pointing both tiers at the same model is a valid choice."
assert_before_plan "Routes are choices, not commitments"
assert_before_plan "Pi owns authentication and model selection"

# Plain style: no emoji anywhere in the framing block.
if head -n $((plan_line - 1)) "$out" | grep -qP '[\x{1F300}-\x{1FAFF}\x{2600}-\x{27BF}]'; then
    loud_fail "emoji found in the framing block"
fi

echo "PASS: concierge prints the operator-framing block before the attended role plan"
