#!/usr/bin/env bash
# Scenario: flip_role_model_routing_honors_config
#
# Pins bug-flip-role-model-routing: when a project configures
# [models.flip_inference] and [models.flip_comparison] to a model distinct
# from the task's runtime model, `wg evaluate run <task> --flip --dry-run`
# MUST report the configured FLIP role models (not the task model) in both
# stdout and the `FLIP models:` stderr line that carries source attribution.
#
# Pre-fix the recorded metadata used CLI > task-model > config, so the
# task model silently shadowed the configured role models whenever a task
# had a runtime model (almost always). The actual LLM calls already routed
# correctly via run_lightweight_llm_call; only the metadata was wrong.
#
# Fast (no real LLM call) — uses --dry-run which prints resolved models
# without making any network requests.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

# Build a minimal graph + config with distinct FLIP role models.
mkdir -p .wg

# Config: flip_enabled with flip_inference/flip_comparison distinct from task model
cat > .wg/config.toml << 'CFG'
[agency]
flip_enabled = true

[models.flip_inference]
model = "openrouter:deepseek/deepseek-v4-flash"

[models.flip_comparison]
model = "openrouter:deepseek/deepseek-v4-flash"
CFG

# Build graph.jsonl with a done task that has a spawn-log entry recording
# a runtime model distinct from the FLIP role models.
python3 -c "
import json
task = {
    'kind': 'task',
    'id': 'a',
    'title': 'Smoke flip routing',
    'status': 'Done',
    'log': [
        {
            'timestamp': '',
            'actor': None,
            'user': None,
            'message': 'Spawned by coordinator --executor claude --model openrouter:z-ai/glm-5.2'
        }
    ]
}
print(json.dumps(task))
" > .wg/graph.jsonl

# Run FLIP dry-run and capture output.
out=$(wg evaluate run a --flip --dry-run 2>.stderr)
rc=$?
stderr=$(cat .stderr)

if [[ $rc -ne 0 ]]; then
    loud_fail "wg evaluate run --flip --dry-run exited $rc. stderr: $stderr"
fi

# The configured role model must appear in stdout.
if ! echo "$out" | grep -q 'Inference model: openrouter:deepseek/deepseek-v4-flash'; then
    loud_fail "FLIP inference model should be the configured role model, not the task model. stdout: $out"
fi
if ! echo "$out" | grep -q 'Comparison model: openrouter:deepseek/deepseek-v4-flash'; then
    loud_fail "FLIP comparison model should be the configured role model, not the task model. stdout: $out"
fi

# The task model must NOT appear as the FLIP models.
if echo "$out" | grep -q 'Inference model: openrouter:z-ai/glm-5.2'; then
    loud_fail "FLIP inference model leaked the task model. stdout: $out"
fi

# stderr carries the source attribution as role/config (not task-model).
if ! echo "$stderr" | grep -q "inference='openrouter:deepseek/deepseek-v4-flash' (role/config)"; then
    loud_fail "FLIP inference source should be role/config. stderr: $stderr"
fi
if ! echo "$stderr" | grep -q "comparison='openrouter:deepseek/deepseek-v4-flash' (role/config)"; then
    loud_fail "FLIP comparison source should be role/config. stderr: $stderr"
fi

echo "PASS: FLIP --dry-run honored [models.flip_*] over task model (inference+comparison=deepseek/deepseek-v4-flash, source=role/config)"
exit 0
