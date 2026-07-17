#!/usr/bin/env bash
# Scenario: tui_chat_mosh_plain_enter_pty
#
# Models the observed mosh failure at the real PTY boundary. The outer session
# carries mosh markers while TERM advertises Kitty support. A physical plain
# Enter corrupted in transit is represented by the exact observed CSI-u bytes
# for Shift+Enter (ESC [ 13 ; 2 u). WG must skip Kitty negotiation, trace that
# parsed event on the native composer route, normalize it centrally, and submit
# every payload exactly once. Ctrl+J remains the reliable multiline fallback.

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
if ! wg chat new --name mosh-enter --command "cat" >chat.log 2>&1; then
    loud_fail "create credential-free cat chat failed: $(cat chat.log)"
fi

graph_dir="$(graph_dir_in "$scratch")" || loud_fail "no .wg dir under $scratch"
trace_path="$scratch/tui-trace.jsonl"

if ! python3 - "$graph_dir" "$trace_path" <<'PY'; then
import fcntl
import json
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
KITTY_QUERY = b"\x1b[?u\x1b[c"
KITTY_PUSH = b"\x1b[>1u"
KITTY_SET = b"\x1b[=1;1u"
CORRUPTED_PLAIN_ENTER = b"\x1b[13;2u"
CTRL_O = b"\x0f"
CTRL_J = b"\x0a"
ENTER = b"\r"

# These markers are inherited when mosh starts or attaches tmux. TERM still
# advertises a Kitty-capable terminal beyond mosh; policy must trust the outer
# transport, not TERM or tmux's extended-key capability.
env = os.environ.copy()
env["TERM"] = "xterm-kitty"
env["MOSH_SERVER_PID"] = "4242"
env["MOSH_IP"] = "192.0.2.10"
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


def fail(msg):
    sys.stderr.write(msg + "\n")
    teardown(1)


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
        return json.loads(out)
    except Exception:
        return {}


def wait_dump_field(field, value, timeout=6.0):
    end = time.time() + timeout
    while time.time() < end:
        if dump().get(field) == value:
            return True
        drain(0.1)
    return False


def persisted_streams():
    streams = []
    for path in (
        graph_dir / "chat" / "0" / "inbox.jsonl",
        graph_dir / "chat-history-0.jsonl",
    ):
        if not path.exists():
            continue
        values = []
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
        streams.append((str(path), values))
    return streams


def persisted_contents():
    streams = persisted_streams()
    return streams[0][1] if streams else []


def wait_for_value(value, timeout=5.0):
    end = time.time() + timeout
    while time.time() < end:
        if value in persisted_contents():
            return True
        drain(0.1)
    return False


drain(4.0)
startup = bytes(buf)
if ALT_SCREEN not in startup:
    fail(f"wg tui never entered alternate screen; captured {len(startup)} bytes")
if KITTY_QUERY in startup:
    fail("mosh policy issued a blocking Kitty capability query")
if KITTY_PUSH in startup or KITTY_SET in startup:
    fail("mosh policy enabled/reasserted Kitty keyboard enhancement")

# Escape the embedded cat/vendor PTY with the canonical host chord, then enter
# the native composer. This ensures the regression is tested on the exact route
# where Shift+Enter means newline.
os.write(fd, CTRL_O)
drain(0.3)
os.write(fd, b"c")
if not wait_dump_field("input_mode", "ChatInput"):
    fail(f"could not enter native ChatInput; last dump={dump()!r}")

# Intentional multiline input remains available without a Shift distinction.
os.write(fd, b"ctrlj-left")
os.write(fd, CTRL_J)
os.write(fd, b"ctrlj-right")
drain(0.2)
if persisted_contents():
    fail(f"Ctrl+J submitted early instead of inserting newline: {persisted_contents()!r}")
multiline = "ctrlj-left\nctrlj-right"
os.write(fd, ENTER)
if not wait_for_value(multiline):
    fail(f"Ctrl+J fallback message was not submitted: {persisted_contents()!r}")

# Exercise repeated literal plain Enter bytes first, then the exact enhanced
# sequence mosh intermittently surfaced for the same physical key. Both cohorts
# must take the same submit-once route.
plain_payloads = [f"plain-enter-{i:02d}" for i in range(12)]
for payload in plain_payloads:
    os.write(fd, payload.encode())
    drain(0.03)
    os.write(fd, ENTER)
    if not wait_for_value(payload):
        fail(f"plain CR Enter did not submit {payload!r}: {persisted_contents()!r}")

corrupt_payloads = [f"mosh-corrupt-{i:02d}" for i in range(12)]
for payload in corrupt_payloads:
    os.write(fd, payload.encode())
    drain(0.03)
    os.write(fd, CORRUPTED_PLAIN_ENTER)
    if not wait_for_value(payload):
        fail(f"misclassified plain Enter did not submit {payload!r}: {persisted_contents()!r}")

# Allow any accidentally queued CR/LF or duplicate event to arrive late.
drain(1.0)
streams = persisted_streams()
if not streams:
    fail("no chat persistence stream was created")
expected_submits = plain_payloads + corrupt_payloads
for path, values in streams:
    observed = [
        v for v in values
        if v.startswith("plain-enter-") or v.startswith("mosh-corrupt-")
    ]
    if observed != expected_submits:
        fail(f"plain Enter submissions were not exactly-once/in-order in {path}: {observed!r}")
    if any("\n" in value for value in observed):
        fail(f"plain Enter inserted a newline in {path}: {observed!r}")
    if values.count(multiline) != 1:
        fail(f"Ctrl+J multiline message duplicated or disappeared in {path}: {values!r}")

# Tracing records the pre-normalization crossterm event plus its active route.
# This is the executable root-cause capture: parsed Shift, native composer.
drain(0.5)
entries = []
if trace_path.exists():
    for line in trace_path.read_text(errors="replace").splitlines():
        try:
            entries.append(json.loads(line))
        except Exception:
            pass
shift_enters = [
    e for e in entries
    if e.get("event", {}).get("type") == "Key"
    and e.get("event", {}).get("code") == "Enter"
    and e.get("event", {}).get("modifiers") == "Shift"
    and e.get("state", {}).get("chat_input_route") == "native_composer"
    and e.get("state", {}).get("input_mode") == "ChatInput"
]
plain_enters = [
    e for e in entries
    if e.get("event", {}).get("type") == "Key"
    and e.get("event", {}).get("code") == "Enter"
    and e.get("event", {}).get("modifiers") == ""
    and e.get("state", {}).get("chat_input_route") == "native_composer"
]
if len(shift_enters) != len(corrupt_payloads):
    fail(
        "trace did not capture every parsed Shift+Enter on native_composer "
        f"(expected {len(corrupt_payloads)}, got {len(shift_enters)})"
    )
# Includes the Enter that submits the Ctrl+J multiline draft.
if len(plain_enters) < len(plain_payloads) + 1:
    fail(
        "trace did not capture repeated plain Enter on native_composer "
        f"(expected at least {len(plain_payloads) + 1}, got {len(plain_enters)})"
    )

sys.stderr.write(
    "PASS: mosh skipped Kitty negotiation; 12 plain CR and 12 parsed "
    "Shift+Enter events on native_composer each submitted once; Ctrl+J "
    "preserved multiline input\n"
)
teardown(0)
PY
    loud_fail "mosh PTY plain-Enter assertions failed"
fi

echo "=== tui_chat_mosh_plain_enter_pty: PASS ==="
