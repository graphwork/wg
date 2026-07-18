#!/usr/bin/env bash
# Live terminal regression for integrate-luca-detached.
# Drives the human `/bg` slash-command flow through a real PTY and proves a
# TERM-ignoring descendant is removed by `/bg kill`, even though no Child
# handle is retained by the command caller.

set -euo pipefail
source "$(dirname "$0")/_helpers.sh"

require_wg
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON" "python3 is required for the PTY harness"

scratch="$(make_scratch)"
(cd "$scratch" && wg init --no-agency >/dev/null)

python3 - "$scratch" <<'PY'
import errno
import fcntl
import glob
import json
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import termios
import time

scratch = sys.argv[1]
ansi = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 120, 0, 0))
attrs = termios.tcgetattr(slave)
attrs[3] &= ~termios.ECHO
termios.tcsetattr(slave, termios.TCSANOW, attrs)

env = os.environ.copy()
env["TERM"] = "xterm-256color"
env["NO_COLOR"] = "1"
env.pop("WG_FAKE_LLM", None)


def child_setup():
    os.setsid()
    try:
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    except OSError:
        pass


proc = subprocess.Popen(
    ["wg", "nex", "--no-mcp", "--max-turns", "4", "-m", "bg-pty-smoke", "-e", "http://127.0.0.1:9"],
    cwd=scratch,
    env=env,
    stdin=slave,
    stdout=slave,
    stderr=slave,
    close_fds=True,
    preexec_fn=child_setup,
)
os.close(slave)
os.set_blocking(master, False)
raw = bytearray()


def clean():
    text = raw.decode("utf-8", errors="replace").replace("\r", "\n")
    return ansi.sub("", text)


def read_some(timeout=0.1):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        ready, _, _ = select.select([master], [], [], 0.05)
        if not ready:
            continue
        try:
            chunk = os.read(master, 65536)
        except OSError as e:
            if e.errno in (errno.EIO, errno.EBADF):
                return
            raise
        if chunk:
            raw.extend(chunk)


def wait_for(needle, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if needle in clean():
            return
        read_some()
    raise RuntimeError(f"timed out waiting for {needle!r}\n--- transcript ---\n{clean()[-5000:]}")


def send(line):
    # Rustyline input is intentionally paced like real terminal typing; one
    # bulk PTY write can race its per-keystroke redraw path on loaded hosts.
    for byte in (line + "\n").encode():
        os.write(master, bytes([byte]))
        time.sleep(0.004)


def pid_alive(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    # Linux kill(0) also sees zombies; zombies cannot execute or be orphans,
    # and should disappear shortly via the runtime/init reaper.
    stat = f"/proc/{pid}/stat"
    try:
        data = open(stat, encoding="utf-8").read()
        end = data.rfind(")")
        return data[end + 2 :].split()[0] != "Z"
    except OSError:
        return True


pgid = None
leader = None
child = None
try:
    wait_for("> ", 15)
    send("/bg run trap '' TERM; /bin/sh -c 'trap \"\" TERM; echo $$ > bg-child.pid; echo BG_READY; while :; do /bin/sleep 1; done' & wait")
    wait_for('"status":"running"', 10)

    match = re.search(r'"id":"(job-[^"]+)"', clean())
    if not match:
        raise RuntimeError(f"could not parse job id\n{clean()[-4000:]}")
    job_id = match.group(1)

    child_path = os.path.join(scratch, "bg-child.pid")
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline and not os.path.exists(child_path):
        time.sleep(0.05)
    child = int(open(child_path, encoding="utf-8").read().strip())

    job_files = glob.glob(os.path.join(scratch, ".wg", "jobs", "*.json"))
    if len(job_files) != 1:
        raise RuntimeError(f"expected one persisted job row, got {job_files}")
    row = json.load(open(job_files[0], encoding="utf-8"))
    leader = int(row["pid"])
    pgid = int(row["process_group"])
    if leader != pgid or not row.get("process_start_identity"):
        raise RuntimeError(f"unsafe persisted containment identity: {row}")
    if not pid_alive(leader) or not pid_alive(child):
        raise RuntimeError("background leader/child was not alive before status")

    send(f"/bg status {job_id}")
    wait_for(f'"id":"{job_id}"', 5)
    send(f"/bg output {job_id} 20")
    wait_for("BG_READY", 5)
    send(f"/bg kill {job_id}")
    wait_for('"status":"cancelled"', 12)

    deadline = time.monotonic() + 4
    while time.monotonic() < deadline and (pid_alive(leader) or pid_alive(child)):
        time.sleep(0.05)
    if pid_alive(leader) or pid_alive(child):
        raise RuntimeError(f"orphan after /bg kill: leader={leader} child={child}\n{clean()[-5000:]}")

    send(f"/cancel {job_id}")
    wait_for('"status":"cancelled"', 5)
    send("/quit")
    proc.wait(timeout=10)
    if proc.returncode != 0:
        raise RuntimeError(f"wg nex exited {proc.returncode}\n{clean()[-5000:]}")
finally:
    # Fail-safe cleanup: the bg session is detached from the PTY session.
    if pgid and (leader is None or pid_alive(leader)):
        try:
            os.killpg(pgid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    if proc.poll() is None:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    try:
        os.close(master)
    except OSError:
        pass

print("nex_bg_process_group_pty: PASS")
PY
