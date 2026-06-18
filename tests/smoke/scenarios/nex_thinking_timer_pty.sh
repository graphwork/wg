#!/usr/bin/env bash
# Scenario: nex_thinking_timer_pty
#
# Pins refresh-nex-thinking: the live `thinking… N tokens` working
# indicator must repaint on a FIXED ~100ms timer, decoupled from token
# arrival, so it animates smoothly and never freezes during prefill or
# mid-stream stalls.
#
# The harness drives `wg nex` through a real PTY against a local SSE
# server that deliberately STALLS: it sends nothing for ~1.5s (prefill),
# then a token, then stalls again, then finishes. With the timer ON the
# `↯ prefilling…` indicator must repaint MANY times during the stall
# (one per tick); with the timer disabled (`WG_NEX_SPINNER_MS=off`, which
# reproduces the pre-fix repaint-on-token-flush-only behavior) it freezes
# at a single paint. A small embedded VT100 emulator replays the captured
# bytes and asserts the cursor-neutral repaint never SCROLLS the screen.
#
# This fails on `main` (no timer → the indicator freezes during the
# stall) and passes after the fix.

set -euo pipefail

source "$(dirname "$0")/_helpers.sh"

require_wg

if ! command -v nex >/dev/null 2>&1; then
    loud_skip "MISSING NEX BINARY" "nex not found on PATH; run 'cargo install --path . --locked' from this checkout"
fi

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the PTY harness"
fi

run_case() {
    local label="$1"
    local spinner_mode="$2"
    local scratch
    scratch="$(make_scratch)"

    (cd "$scratch" && wg init --no-agency >/dev/null)

    if ! python3 - "$scratch" "$label" "$spinner_mode" <<'PY'; then
import errno
import fcntl
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
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

scratch, label, spinner_mode = sys.argv[1:4]

# How long the server withholds output, in seconds. Long enough that a
# ~100ms repaint timer ticks many times (>= REPAINT_MIN), even on a
# loaded CI host.
PREFILL_STALL = 1.5
MIDSTREAM_STALL = 1.5
# With the timer ON we expect ~PREFILL_STALL/0.1 repaints; require a
# conservative floor. With it OFF the indicator is painted once.
REPAINT_MIN_ON = 4
REPAINT_MAX_OFF = 2

ROWS, COLS = 36, 120
ansi_re = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")


def clean(text):
    return ansi_re.sub("", text).replace("\r", "\n")


def fail(message, transcript=""):
    print(f"{label}: {message}", file=sys.stderr)
    if transcript:
        print("---- cleaned transcript tail ----", file=sys.stderr)
        print(clean(transcript)[-1500:], file=sys.stderr)
    raise SystemExit(1)


def sse(handler, obj, delay=0.0):
    handler.wfile.write(("data: " + json.dumps(obj, separators=(",", ":")) + "\n\n").encode())
    handler.wfile.flush()
    if delay:
        time.sleep(delay)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_a):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", "0") or "0")
        self.rfile.read(length)
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("connection", "close")
        self.end_headers()
        # PREFILL STALL — no bytes at all. The indicator should keep
        # animating `↯ prefilling…` purely off the timer.
        time.sleep(PREFILL_STALL)
        sse(self, {"id": "x", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "GO "}, "finish_reason": None}]})
        # MID-STREAM STALL — count is now > 0 but does not change; the
        # `thinking… N tokens` line must keep repainting off the timer.
        time.sleep(MIDSTREAM_STALL)
        for i in range(3):
            sse(self, {"id": "x", "choices": [{"index": 0, "delta": {"content": f"tok{i} "}, "finish_reason": None}]}, delay=0.03)
        sse(self, {"id": "x", "choices": [{"index": 0, "delta": {"content": "DONE_MARKER\n"}, "finish_reason": None}]})
        sse(self, {"id": "x", "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 3, "completion_tokens": 5}})
        try:
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()
        except Exception:
            pass


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
endpoint = f"http://127.0.0.1:{server.server_port}"

master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
attrs = termios.tcgetattr(slave)
attrs[3] &= ~termios.ECHO
termios.tcsetattr(slave, termios.TCSANOW, attrs)

env = os.environ.copy()
env.pop("WG_FAKE_LLM", None)
env.pop("NO_COLOR", None)
env["NEX_DIR"] = os.path.join(scratch, ".nex")
env["WG_NEX_LIVE_INPUT"] = "1"
env["TERM"] = "xterm-256color"
if spinner_mode == "off":
    env["WG_NEX_SPINNER_MS"] = "off"
# else: default cadence (timer on)


def child_setup():
    os.setsid()
    try:
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    except OSError:
        pass


proc = subprocess.Popen(
    ["wg", "nex", "--no-mcp", "--max-turns", "8", "-m", "thinking-timer-smoke", "-e", endpoint],
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


def text():
    return raw.decode("utf-8", errors="replace")


def wait_for(needle, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if needle in clean(text()):
            return
        read_some(0.1)
    fail(f"timed out waiting for {needle!r}", text())


def send_line(line):
    for byte in (line + "\n").encode("utf-8"):
        os.write(master, bytes([byte]))
        time.sleep(0.01)


# --- minimal VT100 replay so we can assert the repaint never scrolls ---
def count_scrolls(data):
    grid_row = 0
    grid_col = 0
    scrolls = 0
    i = 0
    csi = re.compile(r"\x1b\[([0-9;]*)([A-Za-z])")
    while i < len(data):
        ch = data[i]
        if ch == "\x1b" and i + 1 < len(data) and data[i + 1] == "[":
            m = csi.match(data, i)
            if m:
                params, fin = m.group(1), m.group(2)
                first = params.split(";")[0]
                n = int(first) if first else 1
                if fin == "A":
                    grid_row = max(0, grid_row - n)
                elif fin == "B":
                    grid_row = min(ROWS - 1, grid_row + n)
                elif fin == "C":
                    grid_col = min(COLS - 1, grid_col + n)
                elif fin == "D":
                    grid_col = max(0, grid_col - n)
                elif fin in ("H", "f"):
                    parts = params.split(";")
                    grid_row = (int(parts[0]) - 1) if parts and parts[0] else 0
                    grid_col = (int(parts[1]) - 1) if len(parts) > 1 and parts[1] else 0
                i = m.end()
                continue
        if ch == "\r":
            grid_col = 0
        elif ch == "\n":
            grid_row += 1
            if grid_row >= ROWS:
                grid_row = ROWS - 1
                scrolls += 1
        elif ord(ch) >= 32:
            if grid_col >= COLS:
                grid_col = 0
                grid_row += 1
                if grid_row >= ROWS:
                    grid_row = ROWS - 1
                    scrolls += 1
            grid_col += 1
        i += 1
    return scrolls


def prefilling_repaints(data):
    return data.count("prefilling…")


try:
    wait_for(">", 15)
    send_line("hello there")
    # The whole stalled turn is ~PREFILL+MIDSTREAM seconds; read past it.
    wait_for("DONE_MARKER", 20)
    captured = text()

    repaints = prefilling_repaints(captured)
    scrolls = count_scrolls(captured)

    if spinner_mode == "off":
        # Timer disabled == pre-fix behaviour: the prefill indicator is
        # painted at most once and then FROZEN for the whole stall.
        if repaints > REPAINT_MAX_OFF:
            fail(
                f"with the timer OFF the prefill indicator must not repaint "
                f"(saw {repaints}, expected <= {REPAINT_MAX_OFF})",
                captured,
            )
    else:
        # Timer on: the indicator repaints once per ~100ms tick even
        # though ZERO tokens arrived during the prefill stall.
        if repaints < REPAINT_MIN_ON:
            fail(
                f"live thinking indicator did not repaint on the timer during "
                f"the prefill stall (saw {repaints} paints, expected >= "
                f"{REPAINT_MIN_ON}); it froze instead of animating",
                captured,
            )
    # The cursor-neutral repaint must never scroll the screen, regardless
    # of mode.
    if scrolls != 0:
        fail(f"repaint scrolled the screen {scrolls} time(s); it must redraw in place", captured)

    # Drain to the idle prompt and assert the indicator is cleanly gone —
    # no `thinking…`/`prefilling…` trailing the ready `> ` prompt (clean
    # stop at turn end; no leftover repaint).
    wait_for("tok2", 10)
    time.sleep(0.6)
    read_some(0.5)
    final = clean(text())
    idle_matches = list(re.finditer(r"(?m)^[ \t]*>[ \t]*$", final))
    if not idle_matches:
        fail("idle prompt never reappeared after the stalled turn completed", text())
    tail = final[idle_matches[-1].end():]
    if "thinking…" in tail or "prefilling…" in tail:
        fail("live thinking indicator was not cleared at turn end", text())

    send_line("/quit")
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline and proc.poll() is None:
        read_some(0.2)
    if proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        fail("nex did not exit after /quit", text())
    if proc.returncode != 0:
        fail(f"nex exited with {proc.returncode}", text())

    print(f"PASS: {label} (mode={spinner_mode}) prefill repaints={repaints} scrolls={scrolls}")
finally:
    try:
        server.shutdown()
    except Exception:
        pass
    try:
        os.close(master)
    except OSError:
        pass
PY
        loud_fail "$label PTY thinking-timer harness failed (spinner_mode=$spinner_mode)"
    fi
}

# Timer ON (default ~100ms): the indicator must animate through the stall.
run_case "wg nex thinking timer on" default
# Timer OFF (== pre-fix repaint-on-flush-only): the indicator freezes,
# proving the timer is what unfreezes it.
run_case "wg nex thinking timer off" off

echo "PASS: nex live thinking indicator repaints on a fixed timer (and freezes cleanly when disabled), with no screen scroll"
