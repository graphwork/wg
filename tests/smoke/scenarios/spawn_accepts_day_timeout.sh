#!/usr/bin/env bash
# Smoke: `wg add --timeout 1d` is accepted AND the spawn path accepts `1d`.
#
# Regression for fix-wg-add: two timeout parsers disagreed on units. `wg add
# --timeout` validates with graph::parse_delay (s/m/h/d), but the spawn path
# (parse_timeout_secs in src/commands/spawn/mod.rs) only knew s/m/h. So
# `wg add --timeout 1d` was accepted, then the dispatcher/spawn path rejected
# `1d` with "Invalid task timeout value" and the task never spawned — users
# had to rewrite `1d` -> `24h` to unwedge. This pins that both the
# `--timeout` flag and a stored `1d` task timeout now spawn cleanly.
set -eu
source "$(dirname "$0")/_helpers.sh"
require_wg

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME"
wg_dir="$scratch/.wg"

cd "$scratch"
wg init >/dev/null 2>&1 || loud_fail "wg init failed in scratch dir"

# 1) add-time: `--timeout 1d` must be accepted (it always was, but pin it).
if ! wg --dir "$wg_dir" add "tmo-flag" --exec 'echo hi' >/dev/null 2>&1; then
    loud_fail "wg add of tmo-flag failed"
fi
add_out=$(wg --dir "$wg_dir" add "tmo-stored" --exec 'echo hi' --timeout 1d 2>&1) \
    || loud_fail "wg add --timeout 1d rejected at add-time: $add_out"

# 2) spawn path, CLI flag (execution.rs:534). Pre-fix: "Invalid --timeout value".
flag_out=$(wg --dir "$wg_dir" spawn tmo-flag --executor shell --timeout 1d 2>&1) || true
if echo "$flag_out" | grep -qi "invalid"; then
    loud_fail "spawn --timeout 1d rejected: $flag_out"
fi
if ! echo "$flag_out" | grep -qi "spawned"; then
    loud_fail "spawn --timeout 1d did not report a spawned agent: $flag_out"
fi

# 3) spawn path, STORED task timeout (execution.rs:540). Pre-fix:
#    "Invalid task timeout value".
stored_out=$(wg --dir "$wg_dir" spawn tmo-stored --executor shell 2>&1) || true
if echo "$stored_out" | grep -qi "invalid task timeout"; then
    loud_fail "spawn of task with stored 1d timeout rejected: $stored_out"
fi
if ! echo "$stored_out" | grep -qi "spawned"; then
    loud_fail "spawn of task with stored 1d timeout did not spawn: $stored_out"
fi

echo "PASS: spawn_accepts_day_timeout"
