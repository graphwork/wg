#!/usr/bin/env bash
# Regression for fix-wg-daemon: the service daemon must detach from the
# launching shell/session. A terminal hangup after `wg service start` used to
# kill the daemon after tick #1, making `wg service status` clean stale state.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

# The shared helper strips WG_TASK_ID, but service-control also treats a bare
# WG_AGENT_ID as worker context. This scenario intentionally drives the
# terminal/user flow, so remove the remaining worker identity.
unset WG_AGENT_ID

scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME"
cd "$scratch"

if ! wg init --no-agency >init.log 2>&1; then
    loud_fail "wg init failed:
$(cat init.log)"
fi

wg service stop --force --kill-agents >/dev/null 2>&1 || true

# Exercise the real terminal-facing start flow, then simulate the launching
# shell/session receiving SIGHUP. Before the fix the daemon stayed in this
# session and died after the first tick; after the fix it is in its own session
# and keeps ticking.
if ! setsid bash -c '
    wg service start --no-chat-agent --force >start.log 2>&1
    sleep 1
    kill -HUP -$$
    sleep 1
' >/dev/null 2>&1; then
    # The launching shell is expected to die from SIGHUP.
    true
fi

wg_dir="$scratch/.wg"
if ! pid=$(wait_for_daemon_pid "$wg_dir" 5); then
    loud_fail "daemon did not survive long enough to leave a live state file. start log:
$(cat start.log 2>/dev/null || true)

daemon log:
$(cat "$wg_dir/service/daemon.log" 2>/dev/null || true)"
fi
register_wg_daemon "$pid" "$wg_dir"
WG_SMOKE_DAEMON_PID="$pid"
WG_SMOKE_DAEMON_DIR="$wg_dir"

sleep 16

status=$(wg service status 2>&1) || loud_fail "wg service status failed:
$status"

if grep -q "not running" <<<"$status"; then
    loud_fail "daemon exited after launch-session hangup:
$status

start log:
$(cat start.log 2>/dev/null || true)

daemon log:
$(cat "$wg_dir/service/daemon.log" 2>/dev/null || true)"
fi

if ! grep -q "Service: running" <<<"$status"; then
    loud_fail "status did not report a running service:
$status"
fi

if ! grep -qE 'Last tick: .*#[2-9][0-9]*' <<<"$status"; then
    loud_fail "daemon did not complete multiple ticks after hangup:
$status

daemon log:
$(cat "$wg_dir/service/daemon.log" 2>/dev/null || true)"
fi

if ! grep -q "No API key found for provider 'openrouter'" "$wg_dir/service/daemon.log"; then
    loud_fail "fixture did not exercise the known non-fatal OpenRouter key warning:
$(cat "$wg_dir/service/daemon.log" 2>/dev/null || true)"
fi

echo "PASS: service daemon survives launch-session hangup and remains running across multiple ticks"
