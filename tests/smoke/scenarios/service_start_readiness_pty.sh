#!/usr/bin/env bash
# Human-terminal regression for fix-service-start-readiness.
#
# Drives repeated `service stop` -> immediate `service start` pairs through a
# real PTY. Every printed success is followed by an independent nonce/PID
# challenge against the daemon socket. It also proves that a direct startup
# failure remains nonzero and loud on stderr when stdout is redirected.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v script >/dev/null 2>&1 \
    || loud_skip "MISSING PTY DRIVER" "the script(1) command is required"
command -v python3 >/dev/null 2>&1 \
    || loud_skip "MISSING PYTHON3" "python3 is required for the readiness challenge"

unset WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER
# Unix domain sockets have a ~108-byte pathname ceiling. Cargo's isolated
# TMPDIR can itself exceed that, so keep this real-daemon fixture on a short,
# still helper-owned root.
export WG_SMOKE_ROOT="${WG_SMOKE_SHORT_ROOT:-/tmp/wgsmoke}"
scratch=$(make_scratch)
export HOME="$scratch/home"
mkdir -p "$HOME"
wg_dir="$scratch/.wg"
WG_BIN=$(command -v wg)

"$WG_BIN" --dir "$wg_dir" init --no-agency >"$scratch/init.log" 2>&1 \
    || loud_fail "wg init failed: $(cat "$scratch/init.log")"
"$WG_BIN" --dir "$wg_dir" config --local \
    -m pi:openrouter:anthropic/claude-opus-4-7 --no-reload \
    >"$scratch/config.log" 2>&1 \
    || loud_fail "route configuration failed: $(cat "$scratch/config.log")"

# Cleanup through the public lifecycle path; the shared helper's /proc sweep is
# the fallback if an assertion aborts between start and registration.
cleanup_service() {
    "$WG_BIN" --dir "$wg_dir" service stop --force --kill-agents >/dev/null 2>&1 || true
}
add_cleanup_hook cleanup_service

challenge_current_instance() {
    python3 - "$wg_dir/service/state.json" <<'PY'
import json, socket, sys
state_path=sys.argv[1]
with open(state_path, encoding="utf-8") as f:
    state=json.load(f)
nonce=state.get("instance_nonce")
assert nonce, state
request=json.dumps({"cmd":"readiness","instance_nonce":nonce}).encode()+b"\n"
s=socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.settimeout(2)
s.connect(state["socket_path"])
s.sendall(request)
line=b""
while not line.endswith(b"\n"):
    chunk=s.recv(65536)
    assert chunk, "daemon closed readiness connection"
    line += chunk
response=json.loads(line)
assert response.get("ok") is True, response
assert response.get("status") == "ready", response
assert response.get("instance_nonce") == nonce, (state, response)
assert response.get("pid") == state.get("pid"), (state, response)
print(state["pid"], nonce)
PY
}

pty_run() {
    local transcript=$1 command=$2
    # util-linux script -e returns the child status; -f makes each terminal line
    # observable immediately rather than only when the PTY closes.
    script -qefc "$command" "$transcript" </dev/null
}

# Fresh terminal start.
pty_run "$scratch/start-0.pty" \
    "'$WG_BIN' --dir '$wg_dir' service start --no-chat-agent --force"
grep -q "Service started and ready" "$scratch/start-0.pty" \
    || loud_fail "PTY start returned success without an explicit readiness message: $(cat "$scratch/start-0.pty")"
proof=$(challenge_current_instance) \
    || loud_fail "fresh reported success had no matching responsive daemon"
register_wg_daemon "${proof%% *}" "$wg_dir"
prior_nonce=${proof#* }

# Repeat the exact human flow in one PTY shell. `&&` guarantees that a reported
# successful pair means both direct commands returned zero.
for round in 1 2 3 4; do
    transcript="$scratch/stop-start-$round.pty"
    pty_run "$transcript" \
        "'$WG_BIN' --dir '$wg_dir' service stop --force && '$WG_BIN' --dir '$wg_dir' service start --no-chat-agent --force" \
        || loud_fail "PTY stop/start round $round failed: $(cat "$transcript" 2>/dev/null)"
    grep -q "Service stopped" "$transcript" \
        || loud_fail "round $round did not exercise the stop command: $(cat "$transcript")"
    grep -q "Service started and ready" "$transcript" \
        || loud_fail "round $round claimed no readiness-confirmed start: $(cat "$transcript")"
    proof=$(challenge_current_instance) \
        || loud_fail "round $round reported success without a matching responsive daemon"
    pid=${proof%% *}
    nonce=${proof#* }
    [[ "$nonce" != "$prior_nonce" ]] \
        || loud_fail "round $round reused the prior instance nonce ($nonce)"
    prior_nonce=$nonce
    register_wg_daemon "$pid" "$wg_dir"
done

"$WG_BIN" --dir "$wg_dir" service stop --force >/dev/null 2>&1 \
    || loud_fail "could not stop before failure-path check"

# Direct-shell failure semantics: stdout is discarded, so the only evidence a
# human/operator receives is stderr plus the nonzero exit status.
set +e
WG_TEST_SERVICE_START_DELAY_MS=1500 WG_TEST_SERVICE_START_TIMEOUT_MS=150 \
    "$WG_BIN" --dir "$wg_dir" service start --no-chat-agent --no-supervise \
    >/dev/null 2>"$scratch/failure.stderr"
rc=$?
set -e
[[ $rc -ne 0 ]] || loud_fail "readiness timeout returned success with stdout redirected"
grep -q "WG SERVICE START FAILED" "$scratch/failure.stderr" \
    || loud_fail "failure stderr lacked unmistakable heading: $(cat "$scratch/failure.stderr")"
grep -q "readiness timeout" "$scratch/failure.stderr" \
    || loud_fail "failure stderr lacked the reason: $(cat "$scratch/failure.stderr")"
grep -q "Daemon log (last 20 lines)" "$scratch/failure.stderr" \
    || loud_fail "failure stderr lacked bounded log-tail context: $(cat "$scratch/failure.stderr")"
grep -q "Recovery: wg service start --force" "$scratch/failure.stderr" \
    || loud_fail "failure stderr lacked a concrete recovery command: $(cat "$scratch/failure.stderr")"

# JSON remains a single parseable stdout document while the same loud human
# diagnostic independently reaches stderr.
set +e
WG_TEST_SERVICE_START_DELAY_MS=1500 WG_TEST_SERVICE_START_TIMEOUT_MS=150 \
    "$WG_BIN" --dir "$wg_dir" service start --no-chat-agent --no-supervise --json \
    >"$scratch/failure.json" 2>"$scratch/failure-json.stderr"
json_rc=$?
set -e
[[ $json_rc -ne 0 ]] || loud_fail "JSON readiness timeout returned success"
python3 - "$scratch/failure.json" <<'PY' \
    || loud_fail "startup failure stdout was not machine-readable JSON: $(cat "$scratch/failure.json")"
import json, sys
value=json.load(open(sys.argv[1], encoding="utf-8"))
assert value["status"] == "failed", value
assert "readiness timeout" in value["error"], value
assert value["recovery_command"] == "wg service start --force", value
PY
grep -q "WG SERVICE START FAILED" "$scratch/failure-json.stderr" \
    || loud_fail "JSON mode suppressed the loud stderr failure"

echo "PASS: repeated PTY stop/start successes match exact ready daemons; failures are nonzero, loud on stderr, and JSON-safe"
