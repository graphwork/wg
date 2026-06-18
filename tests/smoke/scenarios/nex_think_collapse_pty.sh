#!/usr/bin/env bash
# Scenario: nex_think_collapse_pty
#
# Drives `nex` and `wg nex` through a real PTY against a LOCAL mock
# OpenAI-compatible server (no live model) that streams a reasoning
# `<think>…</think>` block — with both tags fragmented across SSE deltas,
# exactly like Qwen3 / DeepSeek over OpenRouter — followed by the actual
# answer. It asserts the user-visible behavior of collapse-toggle-think:
#
#   * DEFAULT (collapsed): the raw reasoning text is NEVER printed; the
#     answer is shown clean (no `<think>` tags), and the hidden reasoning
#     collapses to a one-line `✓ thought for N tokens` marker.
#   * `/think on` (sent as a REPL command) flips display to raw and a
#     follow-up turn shows the reasoning text verbatim.
#   * `WG_NEX_THINK=1` (env default ON): the very first turn already shows
#     raw reasoning, proving the documented env default works.
#
# This is the human-flow reproducer for collapse-toggle-think: a CLI/unit
# test cannot prove the streaming display path actually suppresses and
# collapses reasoning. The unit + fixture coverage lives in
# src/executor/native/think_filter.rs.

set -euo pipefail

source "$(dirname "$0")/_helpers.sh"

require_wg

if ! command -v nex >/dev/null 2>&1; then
    loud_skip "MISSING NEX BINARY" "nex not found on PATH; run 'cargo install --path . --locked' from this checkout"
fi

if ! command -v python3 >/dev/null 2>&1; then
    loud_skip "MISSING PYTHON" "python3 is required for the PTY harness"
fi

# $1 label, $2 mode (collapse|toggle|env_reveal), then the command + args.
run_case() {
    local label="$1"
    local mode="$2"
    shift 2
    local scratch transcript
    scratch="$(make_scratch)"
    transcript="$scratch/think-pty-${label// /-}.txt"

    (cd "$scratch" && wg init --no-agency >/dev/null)

    if ! python3 - "$scratch" "$transcript" "$label" "$mode" "$@" <<'PY'; then
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

scratch, transcript_path, label, mode = sys.argv[1:5]
cmd = sys.argv[5:]

REASONING_SENTINEL = "REASONING_SENTINEL_should_be_hidden"
ANSWER_MARKER = "ANSWER_MARKER_visible_answer"
ANSWER2_MARKER = "SECOND_ANSWER_MARKER"

ansi_re = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")


def clean(text):
    text = ansi_re.sub("", text)
    text = text.replace("\r", "\n")
    return "".join(ch if ch in "\n\t" or ord(ch) >= 32 else "" for ch in text)


def write_transcript(t):
    with open(transcript_path, "w", encoding="utf-8", errors="replace") as f:
        f.write(t)


def fail(message, transcript=""):
    write_transcript(clean(transcript))
    print(f"{label}: {message}", file=sys.stderr)
    raise SystemExit(1)


def sse(handler, obj, delay=0.02):
    handler.wfile.write(("data: " + json.dumps(obj, separators=(",", ":")) + "\n\n").encode())
    handler.wfile.flush()
    if delay:
        time.sleep(delay)


def done(handler):
    handler.wfile.write(b"data: [DONE]\n\n")
    handler.wfile.flush()


# A reasoning stream whose <think> and </think> tags are split across
# deltas, plus the real answer. `answer` parameterizes the answer text so
# turn 1 and turn 2 differ.
def stream_think(handler, sid, answer):
    for chunk in ("<thi", "nk>", REASONING_SENTINEL + " — ",
                  "weigh the options carefully", "</thi", "nk>"):
        sse(handler, {"id": sid, "choices": [{"index": 0,
            "delta": {"content": chunk}, "finish_reason": None}]})
    sse(handler, {"id": sid, "choices": [{"index": 0,
        "delta": {"content": answer}, "finish_reason": None}]})
    sse(handler, {"id": sid, "choices": [{"index": 0, "delta": {},
        "finish_reason": "stop"}], "usage": {"prompt_tokens": 5,
        "completion_tokens": 9}}, delay=0)
    done(handler)


request_count = [0]


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_):
        pass

    def do_POST(self):
        length = int(self.headers.get("content-length", "0") or "0")
        self.rfile.read(length)
        request_count[0] += 1
        idx = request_count[0]
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("connection", "close")
        self.end_headers()
        answer = ANSWER_MARKER if idx == 1 else ANSWER2_MARKER
        stream_think(self, f"think-{idx}", answer + "\n")


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
endpoint = f"http://127.0.0.1:{server.server_port}"
cmd = [endpoint if part == "__NEX_ENDPOINT__" else part for part in cmd]

master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 36, 120, 0, 0))
attrs = termios.tcgetattr(slave)
attrs[3] &= ~termios.ECHO
termios.tcsetattr(slave, termios.TCSANOW, attrs)

env = os.environ.copy()
env.pop("WG_FAKE_LLM", None)
env["NEX_DIR"] = os.path.join(scratch, ".nex")
env["WG_NEX_LIVE_INPUT"] = "1"
env["TERM"] = "xterm-256color"
env.pop("NO_COLOR", None)
if mode == "env_reveal":
    env["WG_NEX_THINK"] = "1"
else:
    env.pop("WG_NEX_THINK", None)


def child_setup():
    os.setsid()
    try:
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    except OSError:
        pass


proc = subprocess.Popen(cmd, cwd=scratch, env=env, stdin=slave, stdout=slave,
                        stderr=slave, close_fds=True, preexec_fn=child_setup)
os.close(slave)
os.set_blocking(master, False)

raw = bytearray()


def read_some(timeout):
    deadline = time.monotonic() + timeout
    progressed = False
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            try:
                chunk = os.read(master, 65536)
                if chunk:
                    raw.extend(chunk)
                    progressed = True
                    continue
            except OSError:
                pass
            return progressed
        ready, _, _ = select.select([master], [], [], 0.05)
        if not ready:
            continue
        try:
            chunk = os.read(master, 65536)
        except OSError as e:
            if e.errno in (errno.EIO, errno.EBADF):
                return progressed
            raise
        if chunk:
            raw.extend(chunk)
            progressed = True
    return progressed


def text():
    return raw.decode("utf-8", errors="replace")


def wait_for(needle, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if needle in clean(text()):
            return clean(text())
        read_some(0.2)
    fail(f"timed out waiting for {needle!r}", text())


def write_all(payload):
    view = memoryview(payload)
    deadline = time.monotonic() + 5
    while view:
        if time.monotonic() > deadline:
            fail("timed out writing to PTY", text())
        _, writable, _ = select.select([], [master], [], 0.1)
        if not writable:
            continue
        try:
            n = os.write(master, view)
        except BlockingIOError:
            continue
        view = view[n:]


def send_line(line):
    for byte in (line + "\n").encode("utf-8"):
        write_all(bytes([byte]))
        time.sleep(0.01)


try:
    wait_for(">", 15)
    send_line("first prompt please reason")

    after_first = wait_for(ANSWER_MARKER, 30)
    # The collapsed-reasoning marker must appear regardless of mode.
    if "thought for" not in after_first:
        # give the post-turn marker a beat to flush
        time.sleep(0.6)
        read_some(0.5)
        after_first = clean(text())
    if "thought for" not in after_first:
        fail("collapsed reasoning marker '✓ thought for N tokens' never appeared", after_first)

    if mode == "env_reveal":
        # WG_NEX_THINK=1: raw reasoning shown on the FIRST turn already.
        if REASONING_SENTINEL not in after_first:
            fail("WG_NEX_THINK=1 should reveal raw reasoning on the first turn", after_first)
    else:
        # DEFAULT collapsed: raw reasoning must be SUPPRESSED and no
        # `<think>` tag should ever reach the terminal.
        if REASONING_SENTINEL in after_first:
            fail("raw reasoning leaked to the terminal in collapsed (default) mode", after_first)
        if "<think>" in after_first or "</think>" in after_first:
            fail("raw <think> tags leaked to the terminal", after_first)

    if mode == "toggle":
        # Flip display on with the /think REPL command, then a second
        # turn must now show the raw reasoning verbatim.
        time.sleep(0.4)
        read_some(0.3)
        send_line("/think on")
        wait_for("reasoning display ON", 10)
        send_line("second prompt please reason")
        after_second = wait_for(ANSWER2_MARKER, 30)
        if REASONING_SENTINEL not in after_second:
            fail("/think on did not reveal raw reasoning on the next turn", after_second)

    send_line("/quit")
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline and proc.poll() is None:
        read_some(0.2)
    if proc.poll() is None:
        os.killpg(proc.pid, signal.SIGTERM)
        fail("nex did not exit after /quit", text())
    read_some(1)
    final = clean(text())
    write_transcript(final)
    if proc.returncode != 0:
        fail(f"nex exited with {proc.returncode}", final)
    raise SystemExit(0)
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
        loud_fail "$label think-collapse PTY harness failed. Transcript tail:\n$(tail -80 "$transcript" 2>/dev/null || true)"
    fi

    echo "PASS: $label reasoning is collapsed/toggled correctly in the live PTY"
}

run_case "standalone nex collapse" collapse nex --no-mcp --max-turns 8 -m nex-think-collapse-model -e __NEX_ENDPOINT__
run_case "standalone nex toggle"   toggle   nex --no-mcp --max-turns 8 -m nex-think-collapse-model -e __NEX_ENDPOINT__
run_case "wg nex env reveal"       env_reveal wg nex --no-mcp --max-turns 8 -m nex-think-collapse-model -e __NEX_ENDPOINT__

echo "PASS: nex think-block collapse + /think toggle + WG_NEX_THINK default all hold in the live PTY"
