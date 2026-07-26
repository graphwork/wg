#!/usr/bin/env bash
# Regression for impl-maxagents-authority-fix: a runtime `max_agents` override
# (the `--max-agents` launch arg, or a future adaptive-parallelism controller's
# value) must survive a flagless `wg service reload` / profile swap, instead of
# being silently clobbered by `config.coordinator.max_agents`.
#
# See `docs/studies/adaptive-parallelism-budget-design.md` §8. The observed bug
# was "start with `--max-agents 2`, `wg profile use`, observe 8": the launch arg
# was transient (daemon memory only), so a flagless reload re-read config.toml
# and reverted the value.
#
# This scenario drives the REAL human flow (start daemon → reload IPC → read the
# persisted coordinator state) and pins three things the unit tests on
# `handle_reconfigure` alone cannot, because they do not run the coordinator
# tick loop:
#   1. flagless reload preserves the launch-arg override (NOT config's value);
#   2. an explicit `--max-agents` reload flag wins and is recorded as a pin;
#   3. that pin SURVIVES a subsequent coordinator tick (the long-lived in-memory
#      coord_state must not clobber the externally-written disk pin on save).
#
# Credential-free: it routes the daemon at a Pi OpenRouter model string but only
# asserts persisted coordinator-state fields — the endpoint is never contacted
# (the daemon tolerates the "No API key" warning and keeps ticking, exactly like
# service_daemon_survives_launch_session_hangup.sh).
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

# Drive the terminal/user flow, so remove the worker identity.
unset WG_AGENT_ID

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME"
cd "$scratch"

# Unset wg_dir discovery so every `wg` call binds to the scratch fixture.
unset WG_DIR WG_PROJECT_ROOT WG_WORKTREE_PATH WG_WORKTREE_ACTIVE WG_BRANCH WG_TASK_ID

if ! wg init --no-agency >init.log 2>&1; then
    loud_fail "wg init failed:
$(cat init.log)"
fi

wg_dir="$scratch/.wg"

# Seed a route so `wg service start` (which requires an LLM route) succeeds.
# The endpoint is never contacted — the daemon just needs a selected route.
wg --dir "$wg_dir" config --local -m pi:openrouter:anthropic/claude-opus-4-7 --no-reload \
    >config.log 2>&1 || loud_fail "config -m failed:
$(cat config.log)"

# The config value (the "profile wrote 8" baseline) — the ceiling / cold-start
# default. The launch arg below overrides it for the session.
wg --dir "$wg_dir" config set dispatcher.max_agents 8 >>config.log 2>&1 \
    || loud_fail "config set dispatcher.max_agents=8 failed:
$(cat config.log)"

# Fast, deterministic tick cadence so the scenario can sleep past a tick between
# reloads to prove the pin survives the tick save.
wg --dir "$wg_dir" config set dispatcher.poll_interval 3 >>config.log 2>&1 \
    || loud_fail "config set dispatcher.poll_interval=3 failed:
$(cat config.log)"

wg service stop --force --kill-agents >/dev/null 2>&1 || true

# Start the daemon with `--max-agents 2`. This seeds runtime_max_agents = 2
# (the fix) so the launch intent survives a later flagless reload.
start_wg_daemon "$scratch" --no-chat-agent --force --max-agents 2

state_file="$wg_dir/service/coordinator-state-0.json"

read_field() {
    # read_field <field> — echoes the JSON value (or "<missing>").
    python3 - "$state_file" "$1" <<'PY' 2>/dev/null || echo "<missing>"
import json, sys
with open(sys.argv[1]) as f:
    print(json.load(f).get(sys.argv[2], "<missing>"))
PY
}

wait_for_state_file() {
    local i
    for i in $(seq 1 50); do
        [[ -f "$state_file" ]] && return 0
        sleep 0.2
    done
    return 1
}

wait_for_tick_after() {
    # Sleep past one poll_interval (3s) so a coordinator tick fires and saves
    # state AFTER the preceding reload — proving the pin survives the tick.
    sleep 4
}

if ! wait_for_state_file; then
    loud_fail "coordinator state file never appeared. daemon log:
$(cat "$wg_dir/service/daemon.log" 2>/dev/null || true)"
fi

wait_for_tick_after

# --- Step 1: launch arg drove the effective value + seeded a runtime pin. ---
ma=$(read_field max_agents)
rt=$(read_field runtime_max_agents)
if [[ "$ma" != "2" ]]; then
    loud_fail "after start --max-agents 2 (config=8): expected max_agents=2, got '$ma'
daemon log:
$(cat "$wg_dir/service/daemon.log" 2>/dev/null || true)"
fi
if [[ "$rt" != "2" ]]; then
    loud_fail "after start --max-agents 2: expected runtime_max_agents=2 (pin seeded), got '$rt'"
fi

# --- Step 2: a FLAGLESS reload (exactly what `wg profile use` fires) must
#     preserve the runtime override, NOT revert to config's 8. ---
wg --dir "$wg_dir" service reload >reload1.log 2>&1 \
    || loud_fail "flagless reload failed:
$(cat reload1.log)"
wait_for_tick_after
ma=$(read_field max_agents)
rt=$(read_field runtime_max_agents)
if [[ "$ma" != "2" ]]; then
    loud_fail "flagless reload reverted the override: expected max_agents=2, got '$ma' (the original 2->8 bug)"
fi
if [[ "$rt" != "2" ]]; then
    loud_fail "flagless reload lost the runtime pin: expected runtime_max_agents=2, got '$rt'"
fi

# --- Step 3: an explicit `--max-agents` reload flag is a human action — it
#     wins AND is recorded as a pin. Crucially the pin must SURVIVE the next
#     tick (the long-lived in-memory coord_state must not clobber it). ---
wg --dir "$wg_dir" service reload --max-agents 5 >reload2.log 2>&1 \
    || loud_fail "explicit reload --max-agents 5 failed:
$(cat reload2.log)"
wait_for_tick_after
ma=$(read_field max_agents)
rt=$(read_field runtime_max_agents)
if [[ "$ma" != "5" ]]; then
    loud_fail "explicit --max-agents 5 did not win: expected max_agents=5, got '$ma'"
fi
if [[ "$rt" != "5" ]]; then
    loud_fail "explicit --max-agents 5 pin was clobbered by a tick: expected runtime_max_agents=5, got '$rt'"
fi

# --- Step 4: a further flagless reload preserves the human pin. ---
wg --dir "$wg_dir" service reload >reload3.log 2>&1 \
    || loud_fail "second flagless reload failed:
$(cat reload3.log)"
wait_for_tick_after
ma=$(read_field max_agents)
rt=$(read_field runtime_max_agents)
if [[ "$ma" != "5" || "$rt" != "5" ]]; then
    loud_fail "flagless reload after a human pin did not preserve it: expected max_agents=5/runtime=5, got '$ma/$rt'"
fi

echo "PASS: max_agents override survives flagless reload, explicit --max-agents pins + survives a tick"
