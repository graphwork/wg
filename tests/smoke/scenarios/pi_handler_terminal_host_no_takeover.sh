#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=_helpers.sh
source "$SCRIPT_DIR/_helpers.sh"

require_wg
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON" "python3 is required for PTY simulation"

scratch="$(make_scratch)"
bindir="$scratch/bin"
mkdir -p "$bindir"

cat >"$bindir/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail

log="${FAKE_PI_LOG:?FAKE_PI_LOG required}"
args="$*"
stdin_tty=no
stdout_tty=no
if [[ -t 0 ]]; then stdin_tty=yes; fi
if [[ -t 1 ]]; then stdout_tty=yes; fi

has_rpc=no
prev=""
for arg in "$@"; do
    if [[ "$prev" == "--mode" && "$arg" == "rpc" ]]; then
        has_rpc=yes
    fi
    prev="$arg"
done
for arg in "$@"; do
    if [[ "$arg" == "-p" ]]; then
        has_rpc=yes
    fi
done

printf 'START stdin_tty=%s stdout_tty=%s has_rpc=%s args=%s\n' \
    "$stdin_tty" "$stdout_tty" "$has_rpc" "$args" >>"$log"
printf 'PID %s\n' "$$" >>"$log"

if [[ "$stdin_tty" == yes && "$stdout_tty" == yes && "$has_rpc" == no ]]; then
    printf 'TAKEOVER raw-mode-grab\n' >>"$log"
    stty raw -echo 2>/dev/null || true
    sleep 30
    exit 0
fi

printf 'HEADLESS protocol-mode\n' >>"$log"
trap 'printf "TERM\n" >>"$log"; exit 0' TERM
while IFS= read -r line; do
    printf 'LINE %s\n' "$line" >>"$log"
    if [[ "$line" == *'"shutdown"'* ]]; then
        printf 'SHUTDOWN\n' >>"$log"
        exit 0
    fi
done
printf 'EOF\n' >>"$log"
FAKE_PI
chmod +x "$bindir/pi"

(cd "$scratch" && wg init --no-agency >/dev/null)

direct_log="$scratch/direct.log"
if ! timeout 2s python3 - "$bindir" "$direct_log" <<'PY'; then
import os
import pty
import sys

bindir, log = sys.argv[1], sys.argv[2]
env = os.environ.copy()
env["PATH"] = bindir + os.pathsep + env.get("PATH", "")
env["FAKE_PI_LOG"] = log
pid, fd = pty.fork()
if pid == 0:
    os.execvpe("pi", ["pi"], env)
try:
    while True:
        data = os.read(fd, 1024)
        if not data:
            break
except OSError:
    pass
os.waitpid(pid, 0)
PY
    true
fi

grep -q "TAKEOVER raw-mode-grab" "$direct_log" \
    || loud_fail "direct both-TTY/no-flag fake pi did not trip the takeover guard"

handler_log="$scratch/handler-pi.log"
child_pid_file="$scratch/handler.pid"
handler_pty_log="$scratch/handler-pty.log"
export PATH="$bindir:$PATH"
export FAKE_PI_LOG="$handler_log"

python3 - "$scratch/.wg" "$child_pid_file" "$handler_pty_log" <<'PY' &
import os
import pty
import sys

wg_dir, pid_file, pty_log = sys.argv[1], sys.argv[2], sys.argv[3]
argv = [
    "wg",
    "--dir",
    wg_dir,
    "pi-handler",
    "--chat",
    "coordinator-1",
    "--model",
    "pi:openrouter/test/model",
]
pid, fd = pty.fork()
if pid == 0:
    os.execvp("wg", argv)
with open(pid_file, "w", encoding="utf-8") as fh:
    fh.write(str(pid))
with open(pty_log, "ab", buffering=0) as log:
    log.write(("ARGV " + " ".join(argv) + "\n").encode())
    try:
        while True:
            data = os.read(fd, 1024)
            if not data:
                break
            log.write(data)
    except OSError:
        pass
os.waitpid(pid, 0)
PY
pty_wrapper=$!

for _ in $(seq 1 100); do
    if [[ -f "$handler_log" ]] && grep -q "HEADLESS protocol-mode" "$handler_log"; then
        break
    fi
    if ! kill -0 "$pty_wrapper" 2>/dev/null; then
        loud_fail "wg pi-handler PTY wrapper exited before fake pi reached headless mode. pty log:
$(cat "$handler_pty_log" 2>/dev/null || true)
fake pi log:
$(cat "$handler_log" 2>/dev/null || true)"
    fi
    sleep 0.1
done

[[ -f "$handler_log" ]] || loud_fail "fake pi log was not created by wg pi-handler"
grep -q "HEADLESS protocol-mode" "$handler_log" \
    || loud_fail "wg pi-handler did not launch fake pi in protocol/headless mode"
grep -q "stdin_tty=no stdout_tty=no has_rpc=yes" "$handler_log" \
    || loud_fail "fake pi was not spawned with piped stdio and --mode rpc: $(cat "$handler_log")"
if grep -q "TAKEOVER raw-mode-grab" "$handler_log"; then
    loud_fail "wg-hosted pi inherited a PTY and tripped takeover guard: $(cat "$handler_log")"
fi

handler_pid="$(cat "$child_pid_file")"
kill -TERM "$handler_pid" 2>/dev/null || true

for _ in $(seq 1 100); do
    if ! kill -0 "$pty_wrapper" 2>/dev/null; then
        break
    fi
    sleep 0.1
done
if kill -0 "$pty_wrapper" 2>/dev/null; then
    kill -KILL "$pty_wrapper" 2>/dev/null || true
    loud_fail "wg pi-handler did not exit after SIGTERM"
fi
wait "$pty_wrapper" 2>/dev/null || true

for _ in $(seq 1 50); do
    if grep -Eq "EOF|TERM|SHUTDOWN" "$handler_log"; then
        exit 0
    fi
    pi_pid="$(awk '/^PID / {print $2; exit}' "$handler_log" 2>/dev/null || true)"
    if [[ -n "$pi_pid" ]] && ! kill -0 "$pi_pid" 2>/dev/null; then
        exit 0
    fi
    sleep 0.1
done

loud_fail "fake pi child did not observe shutdown/EOF after handler SIGTERM: $(cat "$handler_log")"
