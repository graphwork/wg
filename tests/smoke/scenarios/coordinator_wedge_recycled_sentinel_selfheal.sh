#!/usr/bin/env bash
# Scenario: coordinator_wedge_recycled_sentinel_selfheal
#
# Reproduces the `fix-wedge` incident (observed twice after multi-day daemon
# uptime): a coordinator chat supervisor deferred its own handler respawn
# forever because BOTH the `.tui-driven` sentinel AND the `.handler.pid` lock
# pointed at PIDs that had been RECYCLED by the OS to unrelated live processes.
# The bare `kill(pid, 0)` liveness probe reported "alive" indefinitely, so the
# supervisor logged `TUI sentinel alive (pid=X) — deferring respawn 5s` on a 5s
# loop and never brought a real handler back — until a manual `wg service
# restart`.
#
# The multi-day PID-reuse shape is simulated by pointing both files at live
# `sleep` processes (comm = "sleep", clearly not a wg/nex process). The fix
# (session_lock identity check) reaps the recycled sentinel and recovers the
# recycled lock so the supervisor SELF-HEALS: a fresh real handler respawns and
# the sentinel is cleared. Meanwhile the daemon's dispatcher keeps ticking
# (dispatch is never gated by TUI-sentinel state).
#
# Fails on `main` (supervisor defers forever: no fresh handler, sentinel stays,
# "deferring respawn" spams the log). Passes after the fix.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

fake_llm="$scratch/fake-llm.txt"
printf 'recycled sentinel selfheal smoke response\n' >"$fake_llm"
export WG_FAKE_LLM="$fake_llm"

if ! wg init -m claude:opus --no-agency >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -5 init.log)"
fi

# Short interval so the background dispatcher tick advances quickly within the
# self-heal window. --max-agents 4 leaves headroom so the ready user task is
# NOT withheld by an at-capacity short-circuit (that would report tasks_ready=0
# for a legitimate reason and mask the dispatch-liveness signal we assert).
start_wg_daemon "$scratch" --max-agents 4 --interval 2
wg_dir="$WG_SMOKE_DAEMON_DIR"
daemon_log="$wg_dir/service/daemon.log"

if ! wg service create-chat --name recycled-wedge --exec native --model qwen3-coder >create.log 2>&1; then
    loud_fail "create native chat failed: $(tail -20 create.log)"
fi

chat_dir_for_handler() {
    find "$wg_dir/chat" -mindepth 1 -maxdepth 2 -name '.handler.pid' -print -quit 2>/dev/null \
        | sed 's#/.handler.pid$##'
}

wait_for_handler_dir() {
    local timeout_s="${1:-30}"
    local i dir pid
    for i in $(seq 1 $((timeout_s * 5))); do
        dir="$(chat_dir_for_handler)"
        if [[ -n "$dir" && -f "$dir/.handler.pid" ]]; then
            pid="$(head -1 "$dir/.handler.pid" 2>/dev/null || true)"
            if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
                printf '%s\n' "$dir"
                return 0
            fi
        fi
        sleep 0.2
    done
    return 1
}

if ! chat_dir="$(wait_for_handler_dir 30)"; then
    loud_fail "initial handler never appeared. create.log: $(cat create.log) service files: $(find "$wg_dir/service" -maxdepth 2 -type f -print 2>/dev/null)"
fi

# Two live, unrelated processes standing in for the recycled PIDs.
sleep_a="" sleep_b=""
cleanup_sleeps() {
    [[ -n "${sleep_a:-}" ]] && kill "$sleep_a" >/dev/null 2>&1 || true
    [[ -n "${sleep_b:-}" ]] && kill "$sleep_b" >/dev/null 2>&1 || true
}
add_cleanup_hook cleanup_sleeps

sleep 300 &
sleep_a=$!
sleep 300 &
sleep_b=$!

real_pid="$(head -1 "$chat_dir/.handler.pid" 2>/dev/null || true)"
if [[ -z "$real_pid" ]]; then
    loud_fail "could not read initial handler pid from $chat_dir/.handler.pid"
fi

now_iso="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
# Recycled holder: lock file points at a live foreign PID (SIGKILL'd handler
# whose PID was reused). Recycled sentinel: .tui-driven points at another.
printf '%s\n%s\nchat-nex\n' "$sleep_b" "$now_iso" >"$chat_dir/.handler.pid"
printf '%s\n%s\n' "$sleep_a" "$now_iso" >"$chat_dir/.tui-driven"

# Snapshot dispatcher progress just before we spring the wedge.
pre_ticks=$(grep -c "Coordinator tick #" "$daemon_log" 2>/dev/null || true); pre_ticks=${pre_ticks:-0}

# Now force the supervisor back to the top of its loop so it evaluates the
# planted (recycled) sentinel/holder state. Kill the real handler; SIGKILL
# leaves our overwritten .handler.pid in place.
kill -9 "$real_pid" >/dev/null 2>&1 || true
for _ in $(seq 1 25); do
    if ! kill -0 "$real_pid" 2>/dev/null; then
        break
    fi
    sleep 0.2
done

# A ready user task, added while the supervisor sub-loop is (pre-fix) wedged.
# The dispatcher must still see and process it — dispatch is never gated by
# TUI-sentinel state.
wg --dir "$wg_dir" add "wedge-dispatch-probe" >add.log 2>&1 || true

# Self-heal: a FRESH real handler must respawn with a live wg/nex PID that is
# neither of the recycled sleep PIDs.
new_pid=""
is_wg_process() {
    local pid="$1" comm
    comm="$(cat "/proc/$pid/comm" 2>/dev/null || true)"
    [[ "$comm" == "wg" || "$comm" == "nex" ]]
}
for _ in $(seq 1 150); do
    if [[ -f "$chat_dir/.handler.pid" ]]; then
        candidate="$(head -1 "$chat_dir/.handler.pid" 2>/dev/null || true)"
        if [[ -n "$candidate" ]] \
            && [[ "$candidate" != "$sleep_a" ]] \
            && [[ "$candidate" != "$sleep_b" ]] \
            && kill -0 "$candidate" 2>/dev/null \
            && is_wg_process "$candidate"; then
            new_pid="$candidate"
            break
        fi
    fi
    sleep 0.2
done

if [[ -z "$new_pid" ]]; then
    logs="$(tail -60 "$daemon_log" 2>/dev/null; find "$wg_dir/service" -maxdepth 2 -type f -name '*.log' -exec tail -20 {} \; 2>/dev/null)"
    loud_fail "supervisor did not self-heal: no fresh real handler respawned past the recycled sentinel/holder. chat_dir=$chat_dir logs:\n$logs"
fi

# The recycled sentinel must have been reaped.
if [[ -e "$chat_dir/.tui-driven" ]]; then
    loud_fail "recycled .tui-driven sentinel should have been reaped by the supervisor, but it is still present"
fi

# The "deferring respawn" line must NOT dominate: pre-fix it is logged on a 5s
# loop forever; post-fix the recycled sentinel is reaped so it should not
# repeat. Allow at most one transient occurrence.
defer_lines=$(grep -c "deferring respawn" "$daemon_log" 2>/dev/null || true); defer_lines=${defer_lines:-0}
if (( defer_lines > 1 )); then
    loud_fail "supervisor logged the stale-sentinel deferral $defer_lines times (should be reaped, not looped). log tail:\n$(tail -40 "$daemon_log" 2>/dev/null)"
fi

# Dispatch liveness: the background dispatcher kept ticking through the wedge
# window (it is never blocked by TUI-sentinel state) and saw the ready task.
post_ticks=$(grep -c "Coordinator tick #" "$daemon_log" 2>/dev/null || true); post_ticks=${post_ticks:-0}
if (( post_ticks <= pre_ticks )); then
    loud_fail "dispatcher did not tick during the wedge window (pre=$pre_ticks post=$post_ticks) — dispatch may be starved. log tail:\n$(tail -40 "$daemon_log" 2>/dev/null)"
fi
if ! grep -Eq "Coordinator tick #[0-9]+ complete: .*tasks_ready=[1-9]" "$daemon_log" 2>/dev/null; then
    loud_fail "dispatcher never reported a ready user task while the sentinel wedge was present — dispatch appears gated by TUI-sentinel state. log tail:\n$(tail -40 "$daemon_log" 2>/dev/null)"
fi

echo "PASS: recycled TUI sentinel + handler lock reaped; supervisor self-healed (fresh handler pid=$new_pid), no deferral loop, dispatcher kept ticking (pre=$pre_ticks post=$post_ticks)"
