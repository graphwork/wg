#!/usr/bin/env bash
# Scenario: tui_chat_composer_newlines_pty
#
# Drives the real `wg tui` chat composer through a PTY that observes the direct
# Kitty keyboard-enhancement request. Shift+Enter must insert a newline, not
# submit; Ctrl+J must remain a fallback newline chord; plain Enter must submit
# exactly one multi-line message.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the PTY harness"
fi

scratch=$(make_scratch)
cd "$scratch"

if ! wg init --executor shell >init.log 2>&1; then
    loud_fail "wg init --executor shell failed: $(tail -5 init.log)"
fi

if ! wg chat new --name multiline --command "cat" >chat.log 2>&1; then
    loud_fail "create cat chat failed: $(cat chat.log)"
fi

graph_dir="$(graph_dir_in "$scratch")" || loud_fail "no .wg dir under $scratch after wg init"
trace_path="$scratch/tui-trace.jsonl"

if ! python3 - "$graph_dir" "$trace_path" <<'PY'; then
import json
import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time
from pathlib import Path

graph_dir = Path(sys.argv[1])
trace_path = Path(sys.argv[2])
wg_bin = "wg"

ALT_SCREEN = b"\x1b[?1049h"
KITTY_PUSH = b"\x1b[>1u"
SHIFT_ENTER = b"\x1b[13;2u"
CTRL_O = b"\x0f"
CTRL_J = b"\x0a"
ENTER = b"\r"

env = os.environ.copy()
env["TERM"] = "xterm-kitty"
for var in ("WG_DIR", "WG_PROJECT_ROOT", "WG_WORKTREE_PATH", "WG_TASK_ID"):
    env.pop(var, None)

pid, fd = pty.fork()
if pid == 0:
    os.environ.clear()
    os.environ.update(env)
    os.execvp(
        wg_bin,
        [
            wg_bin,
            "--dir",
            str(graph_dir),
            "tui",
            "--no-mouse",
            "--trace",
            str(trace_path),
        ],
    )
    os._exit(127)

fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 42, 140, 0, 0))
buf = bytearray()


def fail(msg):
    sys.stderr.write(msg + "\n")
    teardown(1)


def teardown(code):
    try:
        os.write(fd, b"q")
        time.sleep(0.1)
        os.write(fd, b"\x03")
    except OSError:
        pass
    try:
        os.kill(pid, signal.SIGKILL)
    except OSError:
        pass
    try:
        os.waitpid(pid, 0)
    except OSError:
        pass
    sys.exit(code)


def drain(seconds):
    end = time.time() + seconds
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if fd not in r:
            continue
        try:
            data = os.read(fd, 65536)
        except OSError:
            return
        if not data:
            return
        buf.extend(data)


def dump():
    try:
        out = subprocess.check_output(
            [wg_bin, "--dir", str(graph_dir), "--json", "tui-dump"],
            stderr=subprocess.DEVNULL,
            timeout=2,
            env=env,
        )
    except Exception:
        return {}
    try:
        return json.loads(out)
    except Exception:
        return {}


def wait_dump_field(field, value, timeout=5.0):
    end = time.time() + timeout
    while time.time() < end:
        d = dump()
        if d.get(field) == value:
            return True
        drain(0.1)
    return False


def persisted_contents():
    values = []
    for path in [
        graph_dir / "chat" / "0" / "inbox.jsonl",
        graph_dir / "chat-history-0.jsonl",
    ]:
        if not path.exists():
            continue
        for line in path.read_text().splitlines():
            if not line.strip():
                continue
            try:
                obj = json.loads(line)
            except Exception:
                continue
            if obj.get("role") in (None, "user", "User"):
                content = obj.get("content") or obj.get("text")
                if isinstance(content, str):
                    values.append(content)
    return values


def wait_persisted(expected, timeout=5.0):
    end = time.time() + timeout
    while time.time() < end:
        values = persisted_contents()
        if values == [expected] or expected in values:
            return values
        drain(0.1)
    return persisted_contents()


drain(8.0)
startup = bytes(buf)
if ALT_SCREEN not in startup:
    fail(f"wg tui never entered alternate screen; captured {len(startup)} bytes")
if KITTY_PUSH not in startup and KITTY_PUSH not in bytes(buf):
    fail("wg tui did not directly request Kitty keyboard disambiguation")

# The cat chat owns the PTY on startup. Ctrl+O is the canonical host escape;
# then `c` enters the native chat composer from graph context.
os.write(fd, CTRL_O)
drain(0.3)
os.write(fd, b"c")
if not wait_dump_field("input_mode", "ChatInput", timeout=6.0):
    fail(f"could not enter ChatInput; last dump={dump()!r}")

os.write(fd, b"shift-one")
drain(0.1)
os.write(fd, SHIFT_ENTER)
drain(0.1)
os.write(fd, b"shift-two")
drain(0.1)

if persisted_contents():
    fail(f"Shift+Enter submitted before plain Enter: {persisted_contents()!r}")

os.write(fd, CTRL_J)
drain(0.1)
os.write(fd, b"ctrlj-three")
drain(0.1)

if persisted_contents():
    fail(f"Ctrl+J fallback submitted before plain Enter: {persisted_contents()!r}")

expected = "shift-one\nshift-two\nctrlj-three"
os.write(fd, ENTER)
values = wait_persisted(expected)
if expected not in values:
    fail(f"plain Enter did not submit exactly one multi-line message; got {values!r}")

if not trace_path.exists() or "Shift" not in trace_path.read_text(errors="ignore"):
    fail("trace did not record a Shift-modified key event from the enhanced PTY")

sys.stderr.write(
    "PASS: enhanced Shift+Enter and Ctrl+J inserted newlines; plain Enter submitted one message\n"
)
teardown(0)
PY
    loud_fail "PTY composer newline assertions failed"
fi

echo "=== tui_chat_composer_newlines_pty: PASS ==="
