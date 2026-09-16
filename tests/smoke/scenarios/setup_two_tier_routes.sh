#!/usr/bin/env bash
# Scenario: setup_two_tier_routes
#
# Pins the two-tier Pi model plane in `wg setup` (setup-collects-strong):
# interactive setup must collect a STRONG route (workers + heavy generative
# roles: task_agent, creator, merger, evolver, verification — reasoning high)
# and a WEAK route (cheap recoverable one-shots: evaluator, assigner,
# flip_inference/flip_comparison, triage, placer, compactors, reviewer —
# reasoning low), explain which roles ride which tier, show the resulting
# role table BEFORE writing, and state that routes stay re-changeable via
# `wg config` / `wg profile select` (Pi owns the model plane).
#
# Contracts asserted here:
#   1. Interactive wizard with two DISTINCT routes writes [tiers] with
#      standard=strong (reasoning high) and fast=weak (reasoning low).
#   2. Interactive wizard with the weak prompt answered by plain Enter
#      (reuse strong) keeps the single-model behavior: no [tiers] written.
#   3. Non-interactive `--yes --weak-model <W>` writes the distinct tiers;
#      `wg config --models` resolves task_agent→strong and evaluator→weak.
#   4. `--yes` WITHOUT --weak-model stays byte-for-byte single-model
#      (unchanged historical behavior).
#   5. `--weak-model` equal to `--model` is treated as single-model.
#
# Interactive steps drive the REAL terminal wizard through a PTY (`script
# -qec`) with piped keystrokes — the same human-flow harness pattern as
# explicit_execution_selection.sh. A bounded `timeout` turns prompt drift
# into a hard FAIL instead of a hang.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)

fake_home="$scratch/home"
mkdir -p "$fake_home" "$scratch/global"
export HOME="$fake_home"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_AGENT_ROLE WG_EXECUTOR_TYPE WG_MODEL WG_TIER 2>/dev/null || true

scenario_step() {
  printf 'SMOKE STEP %s: %s\n' "$1" "$2" >&2
}

STRONG="pi:openrouter:z-ai/glm-5.2"
WEAK="pi:openrouter:deepseek/deepseek-chat"

# Concierge Pi-readiness gates (concierge-guided-pi) sit between the route
# picker and the model wizard. These fixtures isolate $HOME, so live auth
# probes would report not-ready and stop the wizard; inject the all-green
# probe results via the sanctioned WORKSGOOD_PI_GATE_JSON bridge (the same
# convention as WORKSGOOD_PI_MODELS_JSON) so this scenario keeps pinning the
# two-tier prompt flow, not the host's real auth state.
GREEN_PI_GATE_PROBES='{"pi_present":true,"providers":["openrouter"],"authenticated":["openrouter"],"models":["pi:openrouter:z-ai/glm-5.2"]}'

scenario_step 1 'interactive wizard: distinct strong + weak routes'

proj1="$scratch/project-two-tier"
mkdir -p "$proj1/.wg"

# Keystroke sequence (each line = one prompt answer):
#   \n                       → route Select: accept the Pi default
#   $STRONG\n                → STRONG route prompt
#   $WEAK\n                  → WEAK route prompt
#   \n \n \n \n              → agency, max-agents, write-confirm, notify skip
if ! printf '\n%s\n%s\n\n\n\n\n' "$STRONG" "$WEAK" | timeout 90s script -qec \
  "cd '$proj1' && env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER HOME='$fake_home' WG_GLOBAL_DIR='$scratch/global' WORKSGOOD_PI_GATE_JSON='$GREEN_PI_GATE_PROBES' wg --dir '$proj1/.wg' setup" \
  "$scratch/wizard-two-tier.typescript" >/dev/null; then
    loud_fail "interactive setup (two-tier) did not return within 90s (prompt drift / stale key sequence)"
fi

ts="$scratch/wizard-two-tier.typescript"
# The wizard must explain the two tiers and name their roles BEFORE writing.
grep -aq 'STRONG tier' "$ts" || loud_fail "wizard did not explain the STRONG tier:\n$(tail -40 "$ts" | tr -d '\r')"
grep -aq 'task_agent, creator, merger, evolver, verification' "$ts" || loud_fail "wizard did not name the strong-tier roles"
grep -aq 'WEAK tier' "$ts" || loud_fail "wizard did not explain the WEAK tier"
grep -aq 'evaluator, assigner' "$ts" || loud_fail "wizard did not name the weak-tier roles"
grep -aq 'Reusing the strong route for the weak tier is valid' "$ts" || loud_fail "wizard did not offer strong-route reuse for single-model deployments"
# The role table must appear BEFORE the write and label tiers + reasoning.
grep -aq 'Role routing preview' "$ts" || loud_fail "wizard did not show the role routing preview"
grep -aq "effective strong = $STRONG" "$ts" || loud_fail "role preview did not show the effective strong route"
grep -aq "effective weak   = $WEAK" "$ts" || loud_fail "role preview did not show the effective weak route"
# Routes must be presented as re-changeable (Pi owns the model plane).
grep -aq 're-changeable any time' "$ts" || loud_fail "wizard did not state routes are re-changeable"
grep -aq 'wg profile select' "$ts" || loud_fail "wizard did not mention wg profile select"
grep -aq 'Pi owns the model plane' "$ts" || loud_fail "wizard did not state Pi owns the model plane"

cfg1="$proj1/worksgood.toml"
[[ -f "$cfg1" ]] || loud_fail "interactive two-tier setup wrote no worksgood.toml"
grep -qE "^standard = \"$STRONG\"$" "$cfg1" || loud_fail "tiers.standard is not the strong route:\n$(cat "$cfg1")"
grep -qE "^fast = \"$WEAK\"$" "$cfg1" || loud_fail "tiers.fast is not the weak route:\n$(cat "$cfg1")"
grep -qE '^standard_reasoning = "high"' "$cfg1" || loud_fail "strong tier did not get reasoning=high"
grep -qE '^fast_reasoning = "low"' "$cfg1" || loud_fail "weak tier did not get reasoning=low"
# The tiers must actually be DISTINCT (the headline contract).
if [[ "$STRONG" == "$WEAK" ]]; then
    loud_fail "scenario bug: strong and weak fixtures must differ"
fi

scenario_step 2 'interactive wizard: Enter at the weak prompt reuses strong (single-model)'

proj2="$scratch/project-single"
mkdir -p "$proj2/.wg"
# \n route default; Enter accepts the STRONG default; Enter at WEAK = reuse
# strong; then agency / max-agents / write-confirm / notify skip.
if ! printf '\n\n\n\n\n\n\n' | timeout 90s script -qec \
  "cd '$proj2' && env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER HOME='$fake_home' WG_GLOBAL_DIR='$scratch/global' WORKSGOOD_PI_GATE_JSON='$GREEN_PI_GATE_PROBES' wg --dir '$proj2/.wg' setup" \
  "$scratch/wizard-single.typescript" >/dev/null; then
    loud_fail "interactive setup (single-model) did not return within 90s"
fi

cfg2="$proj2/worksgood.toml"
[[ -f "$cfg2" ]] || loud_fail "interactive single-model setup wrote no worksgood.toml"
if grep -qE '^fast = ' "$cfg2"; then
    loud_fail "single-model interactive setup wrote a tiers.fast key:\n$(cat "$cfg2")"
fi
grep -qE "^model = \"$STRONG\"$" "$cfg2" || loud_fail "single-model setup did not pin the strong route as the project default:\n$(cat "$cfg2")"

scenario_step 3 'non-interactive --yes --weak-model writes distinct tiers; wg config --models resolves roles'

proj3="$scratch/project-yes-weak"
mkdir -p "$proj3/.wg"
run_wg3() { (cd "$proj3" && env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER HOME="$fake_home" WG_GLOBAL_DIR="$scratch/global" wg --dir "$proj3/.wg" "$@"); }

run_wg3 setup --route pi --yes --model "$STRONG" --weak-model "$WEAK" \
    >"$scratch/setup-yes-weak.out" 2>&1 || loud_fail "wg setup --route pi --yes --weak-model failed: $(cat "$scratch/setup-yes-weak.out")"

cfg3="$proj3/worksgood.toml"
grep -qE "^standard = \"$STRONG\"$" "$cfg3" || loud_fail "--yes --weak-model did not write tiers.standard:\n$(cat "$cfg3")"
grep -qE "^fast = \"$WEAK\"$" "$cfg3" || loud_fail "--yes --weak-model did not write tiers.fast:\n$(cat "$cfg3")"

# The effective role routing: workers on strong, cheap one-shots on weak.
run_wg3 config --models >"$scratch/models.out" 2>&1 || loud_fail "wg config --models failed"
grep -qE "effective strong *= *$STRONG" "$scratch/models.out" || loud_fail "config --models strong line:\n$(cat "$scratch/models.out")"
grep -qE "effective weak *= *$WEAK" "$scratch/models.out" || loud_fail "config --models weak line:\n$(cat "$scratch/models.out")"
task_row=$(grep -E '^  task_agent ' "$scratch/models.out" || true)
[[ "$task_row" == *"$STRONG"* ]] || loud_fail "task_agent did not resolve to the strong route: '$task_row'"
eval_row=$(grep -E '^  evaluator ' "$scratch/models.out" || true)
[[ "$eval_row" == *"$WEAK"* ]] || loud_fail "evaluator did not resolve to the weak route: '$eval_row'"

scenario_step 4 '--yes without --weak-model stays single-model (unchanged behavior)'

proj4="$scratch/project-yes-plain"
mkdir -p "$proj4/.wg"
run_wg4() { (cd "$proj4" && env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_AGENT_ROLE -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER HOME="$fake_home" WG_GLOBAL_DIR="$scratch/global" wg --dir "$proj4/.wg" "$@"); }

run_wg4 setup --route pi --yes --model "$STRONG" \
    >"$scratch/setup-yes-plain.out" 2>&1 || loud_fail "plain --yes setup failed: $(cat "$scratch/setup-yes-plain.out")"

cfg4="$proj4/worksgood.toml"
if grep -qE '^fast = ' "$cfg4"; then
    loud_fail "plain --yes setup grew a tiers.fast key — the historical single-model paste regressed:\n$(cat "$cfg4")"
fi
grep -qE "^model = \"$STRONG\"$" "$cfg4" || loud_fail "plain --yes setup did not pin the strong route:\n$(cat "$cfg4")"

scenario_step 5 '--weak-model equal to --model is treated as single-model'

run_wg4 setup --route pi --yes --model "$STRONG" --weak-model "$STRONG" \
    >"$scratch/setup-yes-equal.out" 2>&1 || loud_fail "equal weak-model setup failed: $(cat "$scratch/setup-yes-equal.out")"
if grep -qE '^fast = ' "$cfg4"; then
    loud_fail "--weak-model equal to --model wrote a distinct tiers.fast key:\n$(cat "$cfg4")"
fi

scenario_step 6 'two-tier config survives role validation'

run_wg3 config --models >/dev/null 2>&1 || loud_fail "two-tier config failed wg config --models (role plane invalid)"

echo "setup_two_tier_routes: PASS"
