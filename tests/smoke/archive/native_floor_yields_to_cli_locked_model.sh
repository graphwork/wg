#!/usr/bin/env bash
# Scenario: native_floor_yields_to_cli_locked_model
#
# Regression (fix-executor-model / WG_EXECUTOR_MODEL_MISMATCH.md): the handler
# that runs must ALWAYS be derived consistently from the resolved model spec —
# no `(executor, model)` pair that disagrees may reach a spawn.
#
# The original report's symptom was `--executor claude --model gpt-5.5` (a CLI
# that cannot run the model, doom-spawning in ~5s with a generic non-zero exit).
# The residual gap closed here is the mirror image: a `native` (nex-profile)
# dispatcher floor paired with a CLI-locked `claude:`/`codex:` model. The
# in-process nex handler speaks OAI-compat only and cannot run a CLI model, so
# `plan_spawn` must yield to the model spec's handler (`handler_for_model`, the
# single source of truth) instead of doom-spawning on native.
#
# No live LLM endpoint is needed: the spawn log line + agent metadata are
# written before the agent process talks to any model, so a bogus endpoint and
# a 1s timeout are enough to capture the routing decision.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)

# Isolate from any user-level WG config / active profile / global daemon.
fake_home="$scratch/home"
mkdir -p "$fake_home/.config/workgraph"
: >"$fake_home/.config/workgraph/config.toml"

project="$scratch/proj"
mkdir -p "$project"
cd "$project"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" \
        wg "$@"
}

# Native dispatcher floor (the nex profile shape) + a bogus-but-routable
# endpoint so endpoint resolution doesn't bail before the routing decision.
if ! run_wg init -x native -m nex:qwen3-coder-30b \
        -e https://example.invalid/v1 --no-agency >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -10 init.log)"
fi

# Each case: (task id, pinned model, expected handler, expected spawn-log model)
check_route() {
    local task_id="$1" model="$2" want_exec="$3" want_model="$4"

    if ! run_wg add "$task_id" --id "$task_id" --model "$model" \
            >"add-$task_id.log" 2>&1; then
        loud_fail "wg add $task_id failed: $(tail -10 "add-$task_id.log")"
    fi

    # Explicit `--executor native` floor pins the nex handler as the floor;
    # the CLI-locked model must override it. 1s timeout — we only need the
    # spawn log + metadata, both written before the agent runs.
    if ! run_wg spawn "$task_id" --executor native --timeout 1s \
            >"spawn-$task_id.log" 2>&1; then
        loud_fail "wg spawn $task_id failed: $(tail -20 "spawn-$task_id.log")"
    fi

    run_wg show "$task_id" >"show-$task_id.log" 2>&1 || \
        loud_fail "wg show $task_id failed: $(tail -10 "show-$task_id.log")"

    if grep -qE "Spawned by .* --executor native " "show-$task_id.log"; then
        loud_fail "REGRESSION: $task_id ($model) spawned on the native handler it cannot run — the doomed pairing the fix removes: $(grep -i 'Spawned by' "show-$task_id.log")"
    fi
    if ! grep -qE "Spawned by .* --executor ${want_exec} --model ${want_model}" "show-$task_id.log"; then
        loud_fail "$task_id ($model) must route to --executor ${want_exec} --model ${want_model}: $(grep -i 'Spawned by' "show-$task_id.log")"
    fi

    # Spawned-agent env must match the resolved spec: metadata records the
    # executor + (CLI-bare) model the agent was launched with.
    local agent_id
    agent_id="$(grep -oE 'agent-[0-9]+' "show-$task_id.log" | head -1)"
    [ -n "$agent_id" ] || loud_fail "no agent id for $task_id: $(cat "show-$task_id.log")"
    local metadata="$project/.wg/agents/$agent_id/metadata.json"
    [ -f "$metadata" ] || loud_fail "missing metadata for $agent_id at $metadata"
    grep -q "\"executor\": \"${want_exec}\"" "$metadata" || \
        loud_fail "metadata executor for $task_id must be ${want_exec}: $(cat "$metadata")"
    grep -q "\"model\": \"${want_model}\"" "$metadata" || \
        loud_fail "metadata model for $task_id must be ${want_model}: $(cat "$metadata")"
}

# CLI-backed handlers strip the provider prefix (the CLI gets the bare model id).
check_route "claudepin" "claude:opus"   "claude" "opus"
check_route "codexpin"  "codex:gpt-5.5" "codex"  "gpt-5.5"

echo "PASS: native floor yields to CLI-locked claude:/codex: models (handler derived from model spec)"
