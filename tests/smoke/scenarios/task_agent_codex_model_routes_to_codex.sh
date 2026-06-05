#!/usr/bin/env bash
# Scenario: task_agent_codex_model_routes_to_codex
#
# Regression: role model resolution could return model="gpt-5.5" with
# provider="codex", then dispatcher spawn planning received only the bare
# model. The default/agency Claude executor survived, yielding a doomed
# `--executor claude --model gpt-5.5` spawn.
#
# This smoke exercises the real dispatcher path up to the SpawnPlan log line.
# It does not require real LLM credentials; the assertion happens before the
# spawned Codex process can matter.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
mkdir -p "$fake_home/.config/workgraph"
: >"$fake_home/.config/workgraph/config.toml"

cd "$scratch"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" \
        wg "$@"
}

if ! run_wg init >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -10 init.log)"
fi

if ! run_wg model set task_agent codex:gpt-5.5 >model-set.log 2>&1; then
    loud_fail "wg model set task_agent codex:gpt-5.5 failed: $(tail -10 model-set.log)"
fi

python3 - .wg/config.toml <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
lines = path.read_text().splitlines()
out = []
section = None
for line in lines:
    stripped = line.strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        section = stripped.strip("[]")
    if section == "dispatcher" and stripped.startswith("model ="):
        continue
    out.append(line)
path.write_text("\n".join(out) + "\n")
PY

if ! run_wg add "codex route probe" --id codex-route-probe \
        -d "Smoke probe for task-agent codex routing." \
        >add.log 2>&1; then
    loud_fail "wg add failed: $(tail -10 add.log)"
fi

if ! run_wg publish codex-route-probe --only >publish.log 2>&1; then
    loud_fail "wg publish failed: $(tail -10 publish.log)"
fi

( env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" \
        wg service start --max-agents 1 --no-coordinator-agent --interval 1 \
        >start.log 2>&1 ) &
wrap_pid=$!

graph_dir="$scratch/.wg"
if ! daemon_pid=$(wait_for_daemon_pid "$graph_dir" 30); then
    wait "$wrap_pid" 2>/dev/null || true
    loud_fail "daemon never wrote state.json. start.log:\n$(tail -20 start.log 2>/dev/null)"
fi
wait "$wrap_pid" 2>/dev/null || true
register_wg_daemon "$daemon_pid" "$graph_dir"

daemon_log="$graph_dir/service/daemon.log"
spawn_seen=false
for _ in $(seq 1 60); do
    if [[ -f "$daemon_log" ]] \
       && grep -q "codex-route-probe: SpawnPlan" "$daemon_log" 2>/dev/null; then
        spawn_seen=true
        break
    fi
    sleep 0.5
done

spawn_lines=$(grep "codex-route-probe: SpawnPlan" "$daemon_log" 2>/dev/null || true)

if ! $spawn_seen; then
    loud_fail "dispatcher did not emit a SpawnPlan line for codex-route-probe within 30s. Tail of daemon.log:\n$(tail -40 "$daemon_log" 2>/dev/null || echo '<no daemon log>')\nstart.log:\n$(tail -10 start.log 2>/dev/null)"
fi

if grep -qE 'SpawnPlan executor=claude' <<<"$spawn_lines"; then
    loud_fail "codex task-agent route collapsed to executor=claude:\n$spawn_lines"
fi
if ! grep -qE 'SpawnPlan executor=codex' <<<"$spawn_lines"; then
    loud_fail "codex task-agent route did not use executor=codex:\n$spawn_lines"
fi
if ! grep -qE 'model=codex:gpt-5\.5' <<<"$spawn_lines"; then
    loud_fail "SpawnPlan did not preserve provider-qualified model=codex:gpt-5.5:\n$spawn_lines"
fi

echo "PASS: task-agent codex route preserved executor/model atomically (SpawnPlan: $spawn_lines)"
exit 0
