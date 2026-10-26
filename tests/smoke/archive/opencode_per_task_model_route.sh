#!/usr/bin/env bash
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

init_out="$scratch/init.out"
wg init -m claude:opus --no-agency >"$init_out" 2>&1 || \
    loud_fail "wg init failed: $(tail -20 "$init_out")"

add_out="$scratch/add.out"
wg add "OpenCode route smoke" \
    --model opencode:openrouter/stepfun/step-3.7-flash \
    --exec-mode light \
    >"$add_out" 2>&1 || loud_fail "wg add failed: $(cat "$add_out")"

task_id="$(grep -oE '\([A-Za-z0-9._-]+\)' "$add_out" | head -1 | tr -d '()')"
if [ -z "$task_id" ]; then
    loud_fail "could not parse created task id from: $(cat "$add_out")"
fi

if grep -q "openrouter:opencode:openrouter" "$scratch/.wg/graph.jsonl"; then
    loud_fail "wg add stored malformed nested OpenRouter route: $(cat "$scratch/.wg/graph.jsonl")"
fi

grep -q "opencode:openrouter/stepfun/step-3.7-flash" "$scratch/.wg/graph.jsonl" || \
    loud_fail "wg add did not preserve executor-qualified model route: $(cat "$scratch/.wg/graph.jsonl")"

spawn_out="$scratch/spawn.out"
if ! wg spawn "$task_id" --executor native --timeout 1s >"$spawn_out" 2>&1; then
    loud_fail "wg spawn failed before route could be inspected: $(cat "$spawn_out")"
fi

show_out="$scratch/show.out"
wg show "$task_id" >"$show_out" 2>&1 || loud_fail "wg show failed: $(cat "$show_out")"

grep -q "Spawned by .* --executor opencode --model openrouter:stepfun/step-3.7-flash" "$show_out" || \
    loud_fail "spawn log did not show atomic OpenCode route: $(cat "$show_out")"

agent_id="$(grep -oE 'agent-[0-9]+' "$show_out" | head -1)"
if [ -z "$agent_id" ]; then
    loud_fail "could not find spawned agent id in wg show output: $(cat "$show_out")"
fi

metadata="$scratch/.wg/agents/$agent_id/metadata.json"
[ -f "$metadata" ] || loud_fail "missing metadata for $agent_id at $metadata"

grep -q '"executor": "opencode"' "$metadata" || \
    loud_fail "metadata executor was not opencode: $(cat "$metadata")"
grep -q '"model": "openrouter:stepfun/step-3.7-flash"' "$metadata" || \
    loud_fail "metadata model was malformed: $(cat "$metadata")"

run_sh="$scratch/.wg/agents/$agent_id/run.sh"
[ -f "$run_sh" ] || loud_fail "missing run.sh for $agent_id at $run_sh"
grep -q -- "--model 'openrouter/stepfun/step-3.7-flash'" "$run_sh" || \
    loud_fail "OpenCode command did not receive provider/model slash syntax: $(sed -n '1,120p' "$run_sh")"
message_line="$(grep -n "'Complete the attached WG task prompt.'" "$run_sh" | head -1 | cut -d: -f1)"
file_line="$(grep -n -- "--file" "$run_sh" | head -1 | cut -d: -f1)"
[ -n "$message_line" ] && [ -n "$file_line" ] || \
    loud_fail "OpenCode run.sh missing message or --file attachment: $(sed -n '1,120p' "$run_sh")"
[ "$message_line" -le "$file_line" ] || \
    loud_fail "OpenCode message must appear before --file because --file is an array option: $(sed -n '1,120p' "$run_sh")"
if grep -q "openrouter:opencode:openrouter" "$run_sh"; then
    loud_fail "run.sh contains malformed nested route: $(sed -n '1,120p' "$run_sh")"
fi

echo "PASS: opencode per-task model route selects opencode with normalized OpenRouter inner model"
