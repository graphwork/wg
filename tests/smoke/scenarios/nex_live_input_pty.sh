#!/usr/bin/env bash
# Scenario: nex_live_input_pty
#
# Drives standalone `nex` through a real PTY while WG_FAKE_LLM streams
# a deliberately long first response. The PTY has kernel echo disabled;
# this makes the assertion meaningful:
#   * old boundary-only input cannot visibly echo typed text during the
#     stream because no readline prompt owns the terminal at that point
#   * live input renders the queued line through rustyline while the
#     stream continues
#
# The second submitted line must then be consumed as the next turn after
# the first response reaches a safe end-of-turn boundary.

set -euo pipefail

source "$(dirname "$0")/_helpers.sh"

require_wg

if ! command -v nex >/dev/null 2>&1; then
    loud_skip "MISSING NEX BINARY" "nex not found on PATH; run 'cargo install --path . --locked' from this checkout"
fi

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the PTY harness"
fi

scratch="$(make_scratch)"
cd "$scratch"
wg init --no-agency >/dev/null

fake_llm="$scratch/fake_llm.txt"
transcript="$scratch/pty-transcript.txt"

python3 - "$fake_llm" <<'PY'
import sys

path = sys.argv[1]
body = "\n".join(
    f"streaming filler line {i:03d}: live input should remain editable while this arrives"
    for i in range(180)
)
with open(path, "w", encoding="utf-8") as f:
    f.write("FIRST_RESPONSE_MARKER\n")
    f.write(body)
    f.write("\nFIRST_DONE_MARKER\n")
    f.write("---\n")
    f.write("SECOND_RESPONSE_MARKER queued turn received\n")
PY

if ! python3 - "$scratch" "$fake_llm" "$transcript" <<'PY'; then
import errno
import fcntl
import os
import pty
import re
import signal
import struct
import subprocess
import sys
import termios
import time
import select

scratch, fake_llm, transcript_path = sys.argv[1:4]
first_turn = "first live input smoke"
queued_turn = "SECOND_QUEUED_SMOKE while stream active"

ansi_re = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")


def clean(text):
    text = ansi_re.sub("", text)
    text = text.replace("\r", "\n")
    return "".join(ch if ch == "\n" or ch == "\t" or ord(ch) >= 32 else "" for ch in text)


def compact_marker(text):
    return "".join(ch for ch in clean(text) if ch.isalnum() or ch == "_")


def contains_marker(haystack, needle):
    if needle == ">":
        return needle in clean(haystack)
    cleaned = clean(haystack)
    return needle in cleaned or compact_marker(needle) in compact_marker(cleaned)


def fail(message, transcript=""):
    with open(transcript_path, "w", encoding="utf-8", errors="replace") as f:
        f.write(transcript)
    print(message, file=sys.stderr)
    raise SystemExit(1)


master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 120, 0, 0))

# Disable kernel echo before spawning. Live rustyline still paints the
# editable buffer itself; a boundary-only REPL has nothing to paint while
# the model stream is in flight.
attrs = termios.tcgetattr(slave)
attrs[3] &= ~termios.ECHO
termios.tcsetattr(slave, termios.TCSANOW, attrs)

env = os.environ.copy()
env["WG_FAKE_LLM"] = fake_llm
env["WG_NEX_LIVE_INPUT"] = "1"
env.setdefault("TERM", "xterm-256color")


def child_setup():
    os.setsid()
    try:
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    except OSError:
        pass


proc = subprocess.Popen(
    ["nex", "--no-mcp", "--max-turns", "5", "-m", "fake-live-input-model"],
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


def read_some(timeout):
    deadline = time.monotonic() + timeout
    made_progress = False
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            try:
                chunk = os.read(master, 65536)
                if chunk:
                    raw.extend(chunk)
                    made_progress = True
                    continue
            except OSError:
                pass
            return made_progress
        ready, _, _ = select.select([master], [], [], 0.05)
        if not ready:
            continue
        try:
            chunk = os.read(master, 65536)
        except OSError as e:
            if e.errno in (errno.EIO, errno.EBADF):
                return made_progress
            raise
        if chunk:
            raw.extend(chunk)
            made_progress = True
    return made_progress


def text():
    return raw.decode("utf-8", errors="replace")


def wait_for(needle, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if contains_marker(text(), needle):
            return clean(text())
        read_some(0.2)
    fail(f"timed out waiting for {needle!r}", clean(text()))


def write_all(payload):
    view = memoryview(payload)
    deadline = time.monotonic() + 5
    while view:
        if time.monotonic() > deadline:
            fail("timed out writing to PTY", clean(text()))
        _, writable, _ = select.select([], [master], [], 0.1)
        if not writable:
            continue
        try:
            n = os.write(master, view)
        except BlockingIOError:
            continue
        view = view[n:]


def send_line(line):
    for byte in (line + "\n").encode():
        write_all(bytes([byte]))
        time.sleep(0.01)


try:
    wait_for(">", 15)
    send_line(first_turn)

    wait_for("FIRST_RESPONSE_MARKER", 20)
    send_line(queued_turn)

    before_first_done = wait_for("FIRST_DONE_MARKER", 35)
    if compact_marker(queued_turn) not in compact_marker(before_first_done):
        fail(
            "queued input was not visibly preserved in the PTY while the first response streamed",
            before_first_done,
        )

    full = wait_for("SECOND_RESPONSE_MARKER", 35)
    if compact_marker(queued_turn) not in compact_marker(full):
        fail("queued input disappeared from the PTY transcript", full)

    send_line("/quit")
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline and proc.poll() is None:
        read_some(0.2)
    if proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        fail("nex did not exit after queued /quit", clean(text()))
    read_some(1)
    final = clean(text())
    with open(transcript_path, "w", encoding="utf-8", errors="replace") as f:
        f.write(final)
    if proc.returncode != 0:
        fail(f"nex exited with {proc.returncode}", final)
finally:
    try:
        os.close(master)
    except OSError:
        pass
PY
    loud_fail "PTY live-input harness failed. Transcript tail:\n$(tail -80 "$transcript" 2>/dev/null || true)"
fi

journal="$(find "$scratch/.wg/chat" -name conversation.jsonl -print -quit 2>/dev/null || true)"
[[ -n "$journal" ]] || loud_fail "nex did not create a conversation journal"
grep -q "SECOND_QUEUED_SMOKE" "$journal" || loud_fail "queued user turn was not journaled. Journal tail:\n$(tail -20 "$journal")"
grep -q "SECOND_RESPONSE_MARKER" "$journal" || loud_fail "second fake response was not journaled. Journal tail:\n$(tail -20 "$journal")"

echo "PASS: nex PTY accepted visible queued input during streaming and delivered it at the next safe turn"
