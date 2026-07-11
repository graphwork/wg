#!/usr/bin/env bash
# Regression: `wg service start` must detach the daemon from the caller's
# terminal session itself. Closing the real PTY while the start wrapper is
# still alive must SIGHUP only the wrapper's foreground process group; the
# daemon must survive and continue ticking without an external `setsid`.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
scratch=$(make_scratch)
cd "$scratch"
wg init -m claude:opus >init.log 2>&1 || loud_fail "wg init failed: $(tail -10 init.log)"
wg_dir=$(graph_dir_in "$scratch") || loud_fail "wg init did not create a graph"

WG_BIN=$(command -v wg)
export WG_BIN WG_PTY_GRAPH="$wg_dir" WG_PTY_SCRATCH="$scratch"

# forkpty makes the child a session leader with a controlling terminal. Wait
# only until the daemon has recorded its PID, then close the PTY master while
# `service start` is still in its startup wait/spinner. On the regressed build
# the daemon shares that foreground process group and dies from the hangup.
python3 <<'PY'
import json, os, pty, signal, time

wg = os.environ["WG_BIN"]
graph = os.environ["WG_PTY_GRAPH"]
scratch = os.environ["WG_PTY_SCRATCH"]
state = os.path.join(graph, "service", "state.json")
pid, master = pty.fork()
if pid == 0:
    os.chdir(scratch)
    os.execv(wg, [wg, "--dir", graph, "service", "start", "--max-agents", "0",
                  "--no-chat-agent", "--interval", "1"])

deadline = time.monotonic() + 10
daemon_pid = None
while time.monotonic() < deadline:
    try:
        with open(state, encoding="utf-8") as f:
            daemon_pid = json.load(f)["pid"]
        break
    except (FileNotFoundError, json.JSONDecodeError, KeyError):
        time.sleep(0.02)
if daemon_pid is None:
    os.kill(pid, signal.SIGKILL)
    raise SystemExit("service start never wrote state.json")
with open(os.path.join(scratch, "daemon.pid"), "w", encoding="ascii") as f:
    f.write(str(daemon_pid))
# The close is the actual human-flow event under test: terminal/session gone.
os.close(master)
# Reap the launcher if the hangup killed it; never wait on the detached daemon.
for _ in range(50):
    got, _ = os.waitpid(pid, os.WNOHANG)
    if got:
        break
    time.sleep(0.02)
PY

# NOTE: keep this registration before assertions so cleanup also handles a
# partially-working build whose daemon survives but stops ticking.
daemon_pid=$(cat "$scratch/daemon.pid")
register_wg_daemon "$daemon_pid" "$wg_dir"
daemon_log="$wg_dir/service/daemon.log"

proc_is_live() {
    [[ -r "/proc/$1/stat" ]] && [[ "$(awk '{print $3}' "/proc/$1/stat")" != Z ]]
}

sleep 2
proc_is_live "$daemon_pid" || loud_fail "daemon PID $daemon_pid died after its launching PTY closed (start must call setsid internally). log: $(tail -30 "$daemon_log" 2>/dev/null)"
pre=$(grep -c "Coordinator tick #[0-9].* complete" "$daemon_log" 2>/dev/null || true); pre=${pre:-0}
sleep 3
proc_is_live "$daemon_pid" || loud_fail "daemon PID $daemon_pid died during post-hangup poll intervals"
post=$(grep -c "Coordinator tick #[0-9].* complete" "$daemon_log" 2>/dev/null || true); post=${post:-0}
(( post >= pre + 2 )) || loud_fail "daemon survived PTY close but did not tick across multiple poll intervals (pre=$pre post=$post). log: $(tail -40 "$daemon_log" 2>/dev/null)"

echo "PASS: normal wg service start detached internally; daemon $daemon_pid survived PTY hangup and advanced ticks ($pre -> $post)"
