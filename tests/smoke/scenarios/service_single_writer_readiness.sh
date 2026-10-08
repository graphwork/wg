#!/usr/bin/env bash
# Smoke regression for service-start-readiness.
#
# Pins the invariants that made the operator's dev loop untrustworthy:
#   * a readiness probe is served even while the persistent chat agent is still
#     starting (readiness is NOT gated on the chat LLM session);
#   * a readiness timeout never tears down a daemon that is alive by process
#     identity (reported degraded instead of killed);
#   * two concurrent starts can never stack two daemons on one socket;
#   * a stale lock carrier whose owner is dead is reaped, not fatal;
#   * `wg status` derives running/stopped from process identity, never from the
#     state file alone;
#   * the failed-start message gives the actionable remedy instead of the
#     circular `--force` hint, and leaves no orphaned daemon behind.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
# Prefer an explicit candidate, then the freshly-built in-worktree binary
# (the code under test), then the installed `wg`. This keeps `wg done`'s smoke
# gate honest even when the operator's global install lags the worktree.
resolve_wg_bin() {
    if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" && -x "$WG_SMOKE_CANDIDATE_BIN" ]]; then
        printf '%s\n' "$WG_SMOKE_CANDIDATE_BIN"; return 0
    fi
    local root cand
    root="$(cd "$HERE/../../.." && pwd)"
    for cand in "${CARGO_TARGET_DIR:-}/debug/wg" "$root/target/debug/wg" "$root/target/release/wg"; do
        if [[ -n "$cand" && -x "$cand" ]]; then
            printf '%s\n' "$cand"; return 0
        fi
    done
    command -v wg
}
WG_BIN="$(resolve_wg_bin)"
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"
command -v python3 >/dev/null 2>&1 \
    || loud_skip "MISSING PYTHON3" "python3 is required for the readiness/liveness probes"

unset WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER
# Unix domain sockets have a ~108-byte pathname ceiling; keep the fixture short.
export WG_SMOKE_ROOT="${WG_SMOKE_SHORT_ROOT:-/tmp/wgsmoke}"
scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME"
wg_dir="$scratch/.wg"

"$WG_BIN" --dir "$wg_dir" init --no-agency >"$scratch/init.log" 2>&1 \
    || loud_fail "wg init failed: $(cat "$scratch/init.log")"
"$WG_BIN" --dir "$wg_dir" config --local \
    -m pi:openrouter:anthropic/claude-opus-4-7 --no-reload \
    >"$scratch/config.log" 2>&1 \
    || loud_fail "route configuration failed: $(cat "$scratch/config.log")"

stop_daemon() {
    "$WG_BIN" --dir "$wg_dir" service stop --force --kill-agents >/dev/null 2>&1 || true
}
add_cleanup_hook stop_daemon

read_state_pid() {
    grep -oE '"pid"[[:space:]]*:[[:space:]]*[0-9]+' "$wg_dir/service/state.json" \
        | head -1 | grep -oE '[0-9]+$'
}

# Count live `wg service daemon` processes bound to this exact graph dir.
count_daemons() {
    python3 - "$wg_dir" <<'PY'
import os, sys
wg_dir = os.path.realpath(sys.argv[1])
count = 0
for name in os.listdir("/proc"):
    if not name.isdigit():
        continue
    try:
        raw = open(f"/proc/{name}/cmdline", "rb").read()
    except OSError:
        continue
    args = [a for a in raw.split(b"\0") if a]
    if b"service" in args and b"daemon" in args:
        for i in range(len(args) - 1):
            if args[i] == b"--dir" and os.path.realpath(args[i + 1].decode()) == wg_dir:
                count += 1
                break
print(count)
PY
}

challenge_ready() {
    python3 - "$wg_dir/service/state.json" <<'PY'
import json, socket, sys
with open(sys.argv[1], encoding="utf-8") as f:
    state = json.load(f)
nonce = state.get("instance_nonce")
assert nonce, state
request = json.dumps({"cmd": "readiness", "instance_nonce": nonce}).encode() + b"\n"
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(2)
s.connect(state["socket_path"])
s.sendall(request)
line = b""
while not line.endswith(b"\n"):
    chunk = s.recv(65536)
    assert chunk, "daemon closed readiness connection"
    line += chunk
response = json.loads(line)
assert response.get("ok") is True, response
assert response.get("status") == "ready", response
assert response.get("pid") == state.get("pid"), (state, response)
print(state["pid"])
PY
}

# Poll until the currently-recorded daemon answers readiness (used after a
# deliberately-delayed start so the next `service stop` sees a healthy daemon).
await_ready() {
    local i
    for i in $(seq 1 60); do
        if challenge_ready >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

# ── 1. Readiness is not gated on the chat-agent spawn ────────────────────────
"$WG_BIN" --dir "$wg_dir" add ".chat-1" --id .chat-1 -t chat-loop \
    >/dev/null 2>&1 || loud_fail "could not seed a chat-loop task"

start_ns=$(date +%s%N)
WG_TEST_SERVICE_CHAT_BOOT_DELAY_MS=5000 WG_TEST_SERVICE_START_TIMEOUT_MS=8000 \
    "$WG_BIN" --dir "$wg_dir" service start --force >"$scratch/chat-start.log" 2>&1 \
    || loud_fail "chat-enabled start failed: $(cat "$scratch/chat-start.log")"
elapsed_ms=$(( ($(date +%s%N) - start_ns) / 1000000 ))
grep -q "started and ready" "$scratch/chat-start.log" \
    || loud_fail "chat-enabled start did not confirm readiness: $(cat "$scratch/chat-start.log")"
[[ "$elapsed_ms" -lt 5000 ]] \
    || loud_fail "readiness waited for the chat-supervisor boot (${elapsed_ms}ms >= 5000ms)"
chat_pid=$(challenge_ready) \
    || loud_fail "chat-enabled start had no matching responsive daemon"
register_wg_daemon "$chat_pid" "$wg_dir"

# ── 2. A readiness timeout does not kill a live daemon ───────────────────────
stop_daemon
sleep 0.5
rc=0
WG_TEST_SERVICE_START_DELAY_MS=1500 WG_TEST_SERVICE_START_TIMEOUT_MS=150 \
    "$WG_BIN" --dir "$wg_dir" service start --no-chat-agent --no-supervise --force \
    >"$scratch/degraded.log" 2>&1 || rc=$?
[[ $rc -eq 0 ]] \
    || loud_fail "readiness timeout with a live daemon must succeed (rc=$rc): $(cat "$scratch/degraded.log")"
grep -q "readiness was not yet confirmed" "$scratch/degraded.log" \
    || loud_fail "degraded start did not surface the unconfirmed-readiness note: $(cat "$scratch/degraded.log")"
degraded_pid=$(read_state_pid)
kill -0 "$degraded_pid" 2>/dev/null \
    || loud_fail "the live daemon (PID $degraded_pid) was killed after the readiness timeout"
register_wg_daemon "$degraded_pid" "$wg_dir"
await_ready || loud_fail "the delayed daemon never became ready"

# ── 3. Two concurrent starts never stack daemons ─────────────────────────────
stop_daemon
sleep 0.5
"$WG_BIN" --dir "$wg_dir" service start --no-chat-agent >"$scratch/c1.log" 2>&1 &
p1=$!
"$WG_BIN" --dir "$wg_dir" service start --no-chat-agent >"$scratch/c2.log" 2>&1 &
p2=$!
r1=0
wait "$p1" || r1=$?
r2=0
wait "$p2" || r2=$?
[[ $r1 -eq 0 || $r2 -eq 0 ]] \
    || loud_fail "neither concurrent start succeeded: $(cat "$scratch/c1.log") | $(cat "$scratch/c2.log")"
sleep 1
concurrent_count=$(count_daemons)
[[ "$concurrent_count" -eq 1 ]] \
    || loud_fail "expected exactly one daemon after concurrent starts, found $concurrent_count"
concurrent_pid=$(challenge_ready) \
    || loud_fail "the surviving daemon did not answer readiness"
register_wg_daemon "$concurrent_pid" "$wg_dir"

# ── 4. A dead lock owner is reaped, not fatal ────────────────────────────────
stop_daemon
sleep 0.5
dead_pid=$(python3 -c 'import subprocess; p = subprocess.Popen(["/bin/true"]); p.wait(); print(p.pid)')
python3 - "$wg_dir/service/daemon.lock" "$dead_pid" "$wg_dir/service/daemon.sock" <<'PY'
import json, sys
with open(sys.argv[1], "w", encoding="utf-8") as f:
    json.dump({"pid": int(sys.argv[2]), "socket_path": sys.argv[3],
               "started_at": "2026-01-01T00:00:00Z"}, f)
PY
"$WG_BIN" --dir "$wg_dir" service start --no-chat-agent >"$scratch/reap.log" 2>&1 \
    || loud_fail "a dead lock owner blocked the start: $(cat "$scratch/reap.log")"
reap_pid=$(challenge_ready) \
    || loud_fail "start after a stale lock owner produced no responsive daemon"
register_wg_daemon "$reap_pid" "$wg_dir"

# ── 5. `wg status` derives running from process identity ─────────────────────
# Remove state.json while the daemon is alive and dispatching: `wg status` must
# still report running (this is the exact stacked-daemon symptom).
rm -f "$wg_dir/service/state.json"
set +e
status_out=$("$WG_BIN" --dir "$wg_dir" status 2>&1)
set -e
echo "$status_out" | grep -q "Service: running" \
    || loud_fail "wg status reported stopped while a daemon was alive:\n$status_out"
kill -0 "$reap_pid" 2>/dev/null \
    || loud_fail "daemon vanished during the status process-identity check"

# ── 6. Failed-start path: actionable remedy, no orphaned daemon ──────────────
stop_daemon
sleep 0.5
rc=0
WG_TEST_SERVICE_EXIT_BEFORE_READY=1 WG_TEST_SERVICE_START_TIMEOUT_MS=2000 \
    "$WG_BIN" --dir "$wg_dir" service start --no-chat-agent --no-supervise --force \
    >"$scratch/fail.json" 2>"$scratch/fail.stderr" || rc=$?
[[ $rc -ne 0 ]] || loud_fail "a daemon that exits before readiness must fail loudly"
grep -q -- "--no-chat-agent" "$scratch/fail.stderr" \
    || loud_fail "failure stderr lacked the actionable --no-chat-agent remedy: $(cat "$scratch/fail.stderr")"
grep -q "Daemon log" "$scratch/fail.stderr" \
    || loud_fail "failure stderr lacked the daemon-log pointer: $(cat "$scratch/fail.stderr")"
if grep -q "Recovery: wg service start --force" "$scratch/fail.stderr"; then
    loud_fail "the circular --force recovery hint is still present: $(cat "$scratch/fail.stderr")"
fi
sleep 0.5
orphan_count=$(count_daemons)
[[ "$orphan_count" -eq 0 ]] \
    || loud_fail "failed start left $orphan_count orphaned daemon(s) behind"

echo "PASS: chat-agent readiness decoupled; live daemon preserved on timeout; single-writer enforced; status is process-derived; failures are actionable and reap their daemons"
