#!/usr/bin/env bash
# Regression: one-request-per-connection fixed the post-response wait, but an
# accepted client that sends no complete line still parked the daemon's single
# thread in unix_stream_data_wait forever. Exercise partial/malformed clients
# alongside normal Status, Reload, Log/GraphChanged and repeated connections,
# and prove dispatcher safety ticks continue throughout.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg
scratch=$(make_scratch)
cd "$scratch"
wg init -m claude:opus >init.log 2>&1 || loud_fail "wg init failed: $(tail -10 init.log)"
graph_dir=$(graph_dir_in "$scratch") || loud_fail "wg init did not create a graph"
# Reload traffic must preserve the 1s test cadence rather than reverting the
# daemon to the default 5s safety interval.
wg --dir "$graph_dir" config --local --poll-interval 1 --no-reload >/dev/null 2>&1 \
    || loud_fail "failed to configure 1s safety interval"
start_wg_daemon "$scratch" --max-agents 0 --no-chat-agent --interval 1
graph_dir="$WG_SMOKE_DAEMON_DIR"
daemon_pid="$WG_SMOKE_DAEMON_PID"
daemon_log="$graph_dir/service/daemon.log"
socket="$graph_dir/service/daemon.sock"

# Seed a harmless task so `wg log` exercises its real GraphChanged notification.
wg add ipc-stress-probe --no-place >add.log 2>&1 || loud_fail "wg add failed: $(tail -10 add.log)"

for _ in $(seq 1 30); do
    grep -q "Coordinator tick #[0-9].* complete" "$daemon_log" 2>/dev/null && break
    sleep 0.1
done
baseline=$(grep -c "Coordinator tick #[0-9].* complete" "$daemon_log" 2>/dev/null || true); baseline=${baseline:-0}
(( baseline > 0 )) || loud_fail "daemon never completed its initial tick: $(tail -30 "$daemon_log" 2>/dev/null)"

# This is the recurrence trigger that the prior one-request-per-connection fix
# did not cover: accept succeeds, then lines().next() blocks because the peer
# keeps a partial request open without a newline.
python3 - "$socket" <<'PY' &
import socket, sys, time
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(sys.argv[1])
s.sendall(b'{"cmd":"status"')  # deliberately incomplete, no newline
# Hold across several 1s poll intervals. A bounded server must evict us.
time.sleep(4)
s.close()
PY
bad_client=$!

# Wait until the daemon has accepted the client. On the regressed build this
# becomes unix_stream_data_wait; taking the baseline only after that makes the
# pre-fix failure deterministic rather than racing the accept loop.
for _ in $(seq 1 100); do
    [[ "$(cat /proc/$daemon_pid/wchan 2>/dev/null || true)" == "unix_stream_data_wait" ]] && break
    sleep 0.02
done
accepted_wchan=$(cat /proc/$daemon_pid/wchan 2>/dev/null || true)
[[ "$accepted_wchan" == "unix_stream_data_wait" ]] || loud_fail "daemon never accepted partial client (wchan=$accepted_wchan)"
baseline=$(grep -c "Coordinator tick #[0-9].* complete" "$daemon_log" 2>/dev/null || true); baseline=${baseline:-0}
sleep 2
mid=$(grep -c "Coordinator tick #[0-9].* complete" "$daemon_log" 2>/dev/null || true); mid=${mid:-0}
(( mid > baseline )) || loud_fail "partial IPC client stranded coordinator ticks (baseline=$baseline mid=$mid, daemon wchan=$(cat /proc/$daemon_pid/wchan 2>/dev/null || true)). log: $(tail -40 "$daemon_log" 2>/dev/null)"

# Mix real CLI clients with abrupt empty connections, malformed requests, and
# repeated valid requests. Each request uses a fresh connection by contract.
for i in $(seq 1 12); do
    wg --dir "$graph_dir" service status >/dev/null 2>&1 || loud_fail "status client $i failed"
    wg --dir "$graph_dir" log ipc-stress-probe "stress-$i" >/dev/null 2>&1 || loud_fail "log/GraphChanged client $i failed"
    wg --dir "$graph_dir" service reload >/dev/null 2>&1 || loud_fail "reload client $i failed"
    python3 - "$socket" "$i" <<'PY'
import socket, sys
path, i = sys.argv[1], int(sys.argv[2])
# Connect and disappear before sending anything.
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(path); s.close()
# Invalid complete request: daemon should reply/error and close, not retain it.
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(path)
s.sendall(b'not-json\n')
try: s.recv(4096)
except OSError: pass
s.close()
# Normal GraphChanged/Status traffic on independent connections.
for payload in (b'{"cmd":"graph_changed"}\n', b'{"cmd":"status"}\n'):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); s.connect(path)
    s.sendall(payload)
    try: s.recv(65536)
    except OSError: pass
    s.close()
PY
done
wait "$bad_client" 2>/dev/null || true

before_final=$(grep -c "Coordinator tick #[0-9].* complete" "$daemon_log" 2>/dev/null || true); before_final=${before_final:-0}
sleep 3
after_final=$(grep -c "Coordinator tick #[0-9].* complete" "$daemon_log" 2>/dev/null || true); after_final=${after_final:-0}
(( after_final >= before_final + 2 )) || loud_fail "ticks did not continue after mixed IPC clients disconnected (before=$before_final after=$after_final). wchan=$(cat /proc/$daemon_pid/wchan 2>/dev/null || true); log: $(tail -60 "$daemon_log" 2>/dev/null)"
kill -0 "$daemon_pid" 2>/dev/null || loud_fail "daemon exited during IPC stress"

echo "PASS: mixed IPC/status/reload/log/GraphChanged traffic and bad disconnects never stranded daemon ticks ($baseline -> $mid -> $after_final)"
