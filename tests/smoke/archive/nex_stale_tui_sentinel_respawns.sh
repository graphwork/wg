#!/usr/bin/env bash
# Scenario: nex_stale_tui_sentinel_respawns
#
# Reproduces the 2026-06-15 stale `.tui-driven` failure through the real
# daemon supervisor flow. A live sentinel PID with no live `.handler.pid`
# must not suppress respawn forever, and `wg chat resume <id>` must clean up
# the same stale state without manual filesystem edits.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
cd "$scratch"

fake_llm="$scratch/fake-llm.txt"
printf 'stale sentinel smoke response\n' >"$fake_llm"
export WG_FAKE_LLM="$fake_llm"

if ! wg init -m claude:opus --no-agency >init.log 2>&1; then
    loud_fail "wg init failed: $(tail -5 init.log)"
fi

start_wg_daemon "$scratch" --max-agents 1
wg_dir="$WG_SMOKE_DAEMON_DIR"

if ! wg service create-chat --name stale-sentinel --exec native --model qwen3-coder >create.log 2>&1; then
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

sentinel_pid=""
start_live_sentinel() {
    sleep 120 &
    sentinel_pid=$!
    printf '%s\n%s\n' "$sentinel_pid" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$chat_dir/.tui-driven"
}

cleanup_sentinel() {
    if [[ -n "${sentinel_pid:-}" ]]; then
        kill "$sentinel_pid" >/dev/null 2>&1 || true
    fi
}
add_cleanup_hook cleanup_sentinel

make_stale_sentinel_no_handler() {
    local old_pid
    old_pid="$(head -1 "$chat_dir/.handler.pid" 2>/dev/null || true)"
    start_live_sentinel
    printf '{"event":"session_end","reason":"eof","turns":0}\n' >>"$chat_dir/trace.ndjson"
    if [[ -n "$old_pid" ]]; then
        kill "$old_pid" >/dev/null 2>&1 || true
    fi
    for _ in $(seq 1 25); do
        if [[ -z "$old_pid" ]] || ! kill -0 "$old_pid" 2>/dev/null; then
            break
        fi
        sleep 0.2
    done
}

make_stale_sentinel_no_handler

new_pid=""
for _ in $(seq 1 75); do
    if [[ -f "$chat_dir/.handler.pid" ]]; then
        candidate="$(head -1 "$chat_dir/.handler.pid" 2>/dev/null || true)"
        if [[ -n "$candidate" ]] && [[ "$candidate" != "$sentinel_pid" ]] && kill -0 "$candidate" 2>/dev/null; then
            new_pid="$candidate"
            break
        fi
    fi
    sleep 0.2
done

if [[ -z "$new_pid" ]]; then
    logs="$(find "$wg_dir/service" "$scratch" -maxdepth 3 -type f \( -name '*log*' -o -name '*.log' \) -print -exec tail -40 {} \; 2>/dev/null)"
    loud_fail "supervisor did not respawn with live stale .tui-driven and no handler. chat_dir=$chat_dir logs:\n$logs"
fi

if [[ -e "$chat_dir/.tui-driven" ]]; then
    loud_fail "stale .tui-driven should have been cleared by supervisor respawn path"
fi

if grep -R "TUI sentinel alive .*deferring respawn" "$wg_dir/service" "$scratch/daemon.log" >/dev/null 2>&1; then
    loud_fail "supervisor logged stale-sentinel deferral instead of respawning"
fi

# Repeat the stale shape and recover through the user-visible resume command.
kill "$new_pid" >/dev/null 2>&1 || true
for _ in $(seq 1 25); do
    if ! kill -0 "$new_pid" 2>/dev/null; then
        break
    fi
    sleep 0.2
done
make_stale_sentinel_no_handler

if ! wg chat resume 0 >resume.log 2>&1; then
    loud_fail "wg chat resume 0 failed: $(cat resume.log)"
fi

resume_pid=""
for _ in $(seq 1 75); do
    if [[ -f "$chat_dir/.handler.pid" ]]; then
        candidate="$(head -1 "$chat_dir/.handler.pid" 2>/dev/null || true)"
        if [[ -n "$candidate" ]] && kill -0 "$candidate" 2>/dev/null; then
            resume_pid="$candidate"
            break
        fi
    fi
    sleep 0.2
done

if [[ -z "$resume_pid" ]]; then
    loud_fail "wg chat resume did not recover stale sentinel/no-handler state. resume.log: $(cat resume.log)"
fi
if [[ -e "$chat_dir/.tui-driven" ]]; then
    loud_fail "wg chat resume left stale .tui-driven in place"
fi

echo "PASS: stale live TUI sentinel without handler is cleared; supervisor and wg chat resume both respawned Nex handler"
