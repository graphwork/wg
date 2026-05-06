#!/usr/bin/env bash
# Scenario: config_one_line_codex_route
#
# Regression: `wg config --tier ... --tier ...` was rejected by Clap before
# command handling, and the tier handler returned early so a mixed one-line
# config command could not atomically write model, tier, role-routing, FLIP,
# and agency toggles before the reload path.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
mkdir -p "$fake_home/.config"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" \
        wg "$@"
}

help_out=$(run_wg config --help 2>&1) || loud_fail "wg config --help failed: $help_out"
if ! grep -q 'repeat for multiple tiers' <<<"$help_out"; then
    loud_fail "wg config --help does not make --tier repeatability clear. Output:\n$help_out"
fi
if ! grep -q 'repeat for multiple roles' <<<"$help_out"; then
    loud_fail "wg config --help does not make role routing repeatability clear. Output:\n$help_out"
fi
if ! grep -q -- '--dispatcher-model' <<<"$help_out"; then
    loud_fail "wg config --help should advertise canonical --dispatcher-model. Output:\n$help_out"
fi
if grep -q 'coordinator\.model' <<<"$help_out"; then
    loud_fail "wg config --help should not present coordinator.model as the canonical write target. Output:\n$help_out"
fi

project="$scratch/project"
mkdir -p "$project"
cd "$project"

if ! run_wg init --no-agency >"$scratch/init.log" 2>&1; then
    loud_fail "wg init --no-agency failed: $(tail -20 "$scratch/init.log")"
fi

wg_dir=$(graph_dir_in "$project") || loud_fail "no .wg dir after wg init"
config_log="$scratch/config.log"
cfg="$wg_dir/config.toml"

# Simulate an older local config that still pins the deprecated dispatcher
# executor. The one-line model route must be sufficient to clear this stale
# daemon-facing key.
python3 - "$cfg" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
text = path.read_text()
if "[dispatcher]\n" not in text:
    raise SystemExit("missing [dispatcher] section")
path.write_text(text.replace("[dispatcher]\n", "[dispatcher]\nexecutor = \"claude\"\n", 1))
PY

if ! run_wg --dir "$wg_dir" config --local \
    --model codex:gpt-5.5 \
    --coordinator-model codex:gpt-5.5 \
    --tier fast=codex:gpt-5.4-mini \
    --tier standard=codex:gpt-5.4 \
    --tier premium=codex:gpt-5.5 \
    --set-model default codex:gpt-5.5 \
    --set-model task_agent codex:gpt-5.5 \
    --set-model evaluator codex:gpt-5.4-mini \
    --set-model assigner codex:gpt-5.4-mini \
    --flip-model codex:gpt-5.4-mini \
    --auto-assign true \
    --auto-evaluate true >"$config_log" 2>&1; then
    loud_fail "one-line wg config command failed:\n$(cat "$config_log")"
fi

if grep -q "cannot be used multiple times" "$config_log"; then
    loud_fail "one-line wg config command still emitted repeat-flag error:\n$(cat "$config_log")"
fi

[[ -f "$cfg" ]] || loud_fail "missing local config at $cfg"

if ! grep -q 'Set dispatcher.model = "codex:gpt-5.5"' "$config_log"; then
    loud_fail "config output should report canonical dispatcher.model. Output:\n$(cat "$config_log")"
fi
if grep -q 'Set coordinator.model' "$config_log"; then
    loud_fail "config output still reports legacy coordinator.model. Output:\n$(cat "$config_log")"
fi
if ! grep -q 'Cleared deprecated dispatcher.executor' "$config_log"; then
    loud_fail "one-line config did not clear stale dispatcher.executor. Output:\n$(cat "$config_log")"
fi

for expected in \
    'fast = "codex:gpt-5.4-mini"' \
    'standard = "codex:gpt-5.4"' \
    'premium = "codex:gpt-5.5"' \
    'auto_assign = true' \
    'auto_evaluate = true'; do
    if ! grep -q "$expected" "$cfg"; then
        loud_fail "config.toml missing expected line '$expected'. Config:\n$(cat "$cfg")"
    fi
done

assert_section_model() {
    local section="$1"
    local model="$2"
    if ! grep -A3 "^\[$section\]$" "$cfg" | grep -q "model = \"$model\""; then
        loud_fail "section [$section] missing model '$model'. Config:\n$(cat "$cfg")"
    fi
}

assert_section_model "models.default" "codex:gpt-5.5"
assert_section_model "models.task_agent" "codex:gpt-5.5"
assert_section_model "models.evaluator" "codex:gpt-5.4-mini"
assert_section_model "models.assigner" "codex:gpt-5.4-mini"
assert_section_model "models.flip_inference" "codex:gpt-5.4-mini"
assert_section_model "models.flip_comparison" "codex:gpt-5.4-mini"

if grep -qE '^\s*executor\s*=\s*"claude"' "$cfg"; then
    loud_fail "stale dispatcher.executor survived one-line Codex route. Config:\n$(cat "$cfg")"
fi

restart_log="$scratch/restart.log"
if ! run_wg --dir "$wg_dir" service restart >"$restart_log" 2>&1; then
    loud_fail "wg service restart failed after one-line config:\n$(cat "$restart_log")"
fi
if ! pid=$(wait_for_daemon_pid "$wg_dir" 10); then
    loud_fail "service restart did not leave a running daemon. Output:\n$(cat "$restart_log")"
fi
register_wg_daemon "$pid" "$wg_dir"
if ! grep -qE 'Dispatcher: .*executor=codex, model=codex:gpt-5\.5' "$restart_log"; then
    loud_fail "service restart did not resolve codex dispatcher route. Output:\n$(cat "$restart_log")"
fi

echo "PASS: one-line Codex wg config command writes canonical dispatcher.model, clears stale dispatcher.executor, and service restart resolves executor=codex/model=codex:gpt-5.5"
exit 0
