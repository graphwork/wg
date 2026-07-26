#!/usr/bin/env bash
# Live regression for fix-coordinator-daemon: a fatal child exit is loud and
# automatically restarted, while a catchable stray signal is logged/survived.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
scratch=$(make_scratch)
cd "$scratch"
wg init -m pi:openrouter:openai/gpt-4o-mini >init.log 2>&1 || loud_fail "init failed: $(tail -20 init.log)"
wg_dir=$(graph_dir_in "$scratch") || loud_fail "missing graph"
cleanup_daemon() { wg --dir "$wg_dir" service stop --force >/dev/null 2>&1 || true; }
trap cleanup_daemon EXIT
wg --dir "$wg_dir" service start --max-agents 0 --no-chat-agent --interval 1 >start.log 2>&1 || loud_fail "start failed: $(cat start.log)"
state="$wg_dir/service/state.json"
log="$wg_dir/service/daemon.log"
old_pid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$state")
supervisor_pid=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["supervisor_pid"])' "$state")
kill -9 "$old_pid" || loud_fail "could not kill daemon $old_pid"
new_pid=""
for _ in $(seq 1 100); do
  new_pid=$(python3 -c 'import json,sys
try: print(json.load(open(sys.argv[1]))["pid"])
except Exception: pass' "$state" 2>/dev/null)
  if [[ -n "$new_pid" && "$new_pid" != "$old_pid" ]] && kill -0 "$new_pid" 2>/dev/null; then break; fi
  sleep .1
done
[[ -n "$new_pid" && "$new_pid" != "$old_pid" ]] || loud_fail "supervisor did not replace killed daemon; log: $(tail -50 "$log")"
kill -0 "$supervisor_pid" 2>/dev/null || loud_fail "supervisor died instead of restarting child"
grep -q "exited unexpectedly" "$log" || loud_fail "fatal child exit was not logged loudly: $(tail -50 "$log")"
# The historical silent-exit class was default signal disposition. A stray
# SIGHUP must now be observable and non-fatal.
kill -HUP "$new_pid" || loud_fail "could not signal replacement daemon"
sleep 2
kill -0 "$new_pid" 2>/dev/null || loud_fail "replacement daemon died from catchable SIGHUP"
grep -q "Survived stray signal.*SIGHUP" "$log" || loud_fail "SIGHUP was not named in daemon.log: $(tail -50 "$log")"
grep -q "Coordinator tick #[0-9].*complete" "$log" || loud_fail "dispatch ticks did not resume after restart"
wg --dir "$wg_dir" service stop >stop.log 2>&1 || loud_fail "clean stop failed: $(cat stop.log)"
trap - EXIT
for _ in $(seq 1 50); do kill -0 "$supervisor_pid" 2>/dev/null || break; sleep .1; done
kill -0 "$supervisor_pid" 2>/dev/null && loud_fail "clean stop left supervisor alive"
echo "PASS: SIGKILL crash was loud and auto-restarted ($old_pid -> $new_pid); SIGHUP was logged and survived; ticks resumed"
